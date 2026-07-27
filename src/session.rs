use std::io::{self, Write};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use socket2::{Domain, Socket, Type};

use crate::audio::{list_devices, resolve_device_name, start_audio, AudioBuffers};
use crate::codec::OpusCodec;
use crate::protocol::{
    encode_audio, encode_hello, encode_ping, encode_pong, encode_simple, parse, PacketKind,
    ParsedPacket, FRAME_MS, JITTER_BUFFER_FRAMES, PORT_MAC_TO_SERVER, PORT_SERVER_TO_MAC,
    SAMPLE_RATE, TYPE_BYE, TYPE_HELLO_ACK, TYPE_RESUME,
};

#[derive(Default)]
pub struct Stats {
    pub rtt_ms: f64,
    pub jitter_ms: f64,
    pub loss_pct: f64,
    pub tx_kbps: f64,
    pub rx_kbps: f64,
}

struct SessionFlags {
    running: AtomicBool,
    paused: AtomicBool,
    mute_mic: AtomicBool,
    mute_remote: AtomicBool,
    send_seq: AtomicU16,
}

impl SessionFlags {
    fn new() -> Self {
        Self {
            running: AtomicBool::new(true),
            paused: AtomicBool::new(false),
            mute_mic: AtomicBool::new(false),
            mute_remote: AtomicBool::new(false),
            send_seq: AtomicU16::new(0),
        }
    }
}

pub fn run_devices() -> Result<(), Box<dyn std::error::Error>> {
    list_devices()?;
    Ok(())
}

pub fn run_listen(bind_addr: &str, device: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let device = resolve_device_name(device);
    println!("Listening on {bind_addr}:{PORT_MAC_TO_SERVER} (audio in)");
    println!("Sending audio to client:{PORT_SERVER_TO_MAC}");
    if let Some(ref d) = device {
        println!("Audio device filter: {d}");
    }
    print_controls();

    let socket = bind_udp(format!("{bind_addr}:{PORT_MAC_TO_SERVER}"))?;
    let flags = Arc::new(SessionFlags::new());
    let stats = Arc::new(Mutex::new(Stats::default()));
    let peer: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));

    let (_audio_guard, buffers) = start_audio(device.as_deref())?;
    let codec = Arc::new(Mutex::new(OpusCodec::new()?));

    spawn_keyboard(Arc::clone(&flags));
    spawn_audio_pipeline(
        buffers.clone(),
        Arc::clone(&codec),
        Arc::clone(&flags),
        Arc::clone(&stats),
        Arc::clone(&peer),
        socket.try_clone()?,
        Role::Server,
    )?;
    run_recv_loop(
        socket,
        buffers,
        Arc::clone(&codec),
        Arc::clone(&flags),
        Arc::clone(&stats),
        Arc::clone(&peer),
        Role::Server,
    )?;

    disable_raw_mode().ok();
    Ok(())
}

pub fn run_connect(host: &str, device: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let device = resolve_device_name(device);
    let remote: SocketAddr = format!("{host}:{PORT_MAC_TO_SERVER}").parse()?;
    let local_bind = format!("0.0.0.0:{PORT_SERVER_TO_MAC}");

    println!("Connecting to {remote} from {local_bind}");
    if let Some(ref d) = device {
        println!("Audio device filter: {d}");
    }
    print_controls();

    let socket = bind_udp(local_bind)?;
    socket.connect(remote)?;

    let hello = encode_hello(PORT_SERVER_TO_MAC);
    for _ in 0..5 {
        socket.send(&hello)?;
        thread::sleep(Duration::from_millis(50));
    }

    let flags = Arc::new(SessionFlags::new());
    let stats = Arc::new(Mutex::new(Stats::default()));
    let peer = Arc::new(Mutex::new(Some(remote)));

    let (_audio_guard, buffers) = start_audio(device.as_deref())?;
    let codec = Arc::new(Mutex::new(OpusCodec::new()?));

    spawn_keyboard(Arc::clone(&flags));
    spawn_audio_pipeline(
        buffers.clone(),
        Arc::clone(&codec),
        Arc::clone(&flags),
        Arc::clone(&stats),
        Arc::clone(&peer),
        socket.try_clone()?,
        Role::Client,
    )?;
    run_recv_loop(
        socket,
        buffers,
        Arc::clone(&codec),
        Arc::clone(&flags),
        Arc::clone(&stats),
        Arc::clone(&peer),
        Role::Client,
    )?;

    disable_raw_mode().ok();
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Client,
    Server,
}

fn bind_udp(addr: String) -> Result<UdpSocket, io::Error> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, None)?;
    socket.set_reuse_address(true)?;
    let sa: SocketAddr = addr.parse().expect("invalid bind address");
    socket.bind(&sa.into())?;
    Ok(socket.into())
}

fn spawn_keyboard(flags: Arc<SessionFlags>) {
    thread::spawn(move || {
        let _ = enable_raw_mode();
        while flags.running.load(Ordering::Relaxed) {
            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                if let Ok(Event::Key(key)) = event::read() {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    match key.code {
                        KeyCode::Char('m') => {
                            let v = !flags.mute_mic.load(Ordering::Relaxed);
                            flags.mute_mic.store(v, Ordering::Relaxed);
                            eprintln!("\rmic {}", if v { "MUTED" } else { "on   " });
                        }
                        KeyCode::Char('M') => {
                            let v = !flags.mute_remote.load(Ordering::Relaxed);
                            flags.mute_remote.store(v, Ordering::Relaxed);
                            eprintln!("\rremote {}", if v { "MUTED" } else { "on   " });
                        }
                        KeyCode::Char('p') | KeyCode::Char('P') => {
                            let v = !flags.paused.load(Ordering::Relaxed);
                            flags.paused.store(v, Ordering::Relaxed);
                            eprintln!(
                                "\r{}",
                                if v {
                                    "PAUSED (no traffic)"
                                } else {
                                    "RESUMED          "
                                }
                            );
                        }
                        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                            flags.running.store(false, Ordering::Relaxed);
                        }
                        _ => {}
                    }
                }
            }
        }
    });
}

fn spawn_audio_pipeline(
    buffers: AudioBuffers,
    codec: Arc<Mutex<OpusCodec>>,
    flags: Arc<SessionFlags>,
    stats: Arc<Mutex<Stats>>,
    peer: Arc<Mutex<Option<SocketAddr>>>,
    socket: UdpSocket,
    role: Role,
) -> Result<(), Box<dyn std::error::Error>> {
    thread::spawn(move || {
        let mut tx_bytes: u64 = 0;
        let mut window = Instant::now();
        let mut timestamp: u32 = 0;
        let mut ping_at = Instant::now();
        let mut was_paused = false;

        while flags.running.load(Ordering::Relaxed) {
            let paused = flags.paused.load(Ordering::Relaxed);
            if paused {
                was_paused = true;
                thread::sleep(Duration::from_millis(FRAME_MS as u64));
                continue;
            }

            if was_paused {
                was_paused = false;
                if let Some(addr) = *peer.lock().unwrap() {
                    let _ = socket.send_to(&encode_simple(TYPE_RESUME), peer_dest(role, addr));
                }
            }

            if ping_at.elapsed() >= Duration::from_secs(2) {
                if let Some(addr) = *peer.lock().unwrap() {
                    let ping = encode_ping(now_ms());
                    let _ = socket.send_to(&ping, peer_dest(role, addr));
                }
                ping_at = Instant::now();
            }

            if let Some(frame) = buffers.drain_capture_frame() {
                if flags.mute_mic.load(Ordering::Relaxed) {
                    timestamp = timestamp.wrapping_add(FRAME_MS);
                    thread::sleep(Duration::from_millis(FRAME_MS as u64));
                    continue;
                }

                let opus = {
                    let mut c = codec.lock().unwrap();
                    match c.encode(&frame) {
                        Ok(data) => data.to_vec(),
                        Err(e) => {
                            eprintln!("encode error: {e}");
                            continue;
                        }
                    }
                };

                let seq = flags.send_seq.fetch_add(1, Ordering::Relaxed);
                let packet = encode_audio(seq, timestamp, &opus);
                timestamp = timestamp.wrapping_add(FRAME_MS);

                if let Some(addr) = *peer.lock().unwrap() {
                    if socket.send_to(&packet, peer_dest(role, addr)).is_ok() {
                        tx_bytes += packet.len() as u64;
                    }
                }
            } else {
                thread::sleep(Duration::from_millis(2));
            }

            if window.elapsed() >= Duration::from_secs(1) {
                stats.lock().unwrap().tx_kbps = (tx_bytes as f64 * 8.0) / 1000.0;
                tx_bytes = 0;
                window = Instant::now();
            }
        }
    });

    Ok(())
}

fn run_recv_loop(
    socket: UdpSocket,
    buffers: AudioBuffers,
    codec: Arc<Mutex<OpusCodec>>,
    flags: Arc<SessionFlags>,
    stats: Arc<Mutex<Stats>>,
    peer: Arc<Mutex<Option<SocketAddr>>>,
    role: Role,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut rx_bytes: u64 = 0;
    let mut window = Instant::now();
    let mut last_seq: Option<u16> = None;
    let mut lost: u64 = 0;
    let mut received: u64 = 0;
    let mut jitter_buf: Vec<Vec<i16>> = Vec::new();
    let mut primed = false;
    let mut last_arrival = Instant::now();
    let mut jitter_sum = 0.0f64;
    let mut jitter_n = 0u64;
    let t0 = Instant::now();

    while flags.running.load(Ordering::Relaxed) {
        if flags.paused.load(Ordering::Relaxed) {
            buffers.clear_playback();
            jitter_buf.clear();
            primed = false;
            thread::sleep(Duration::from_millis(20));
            print_status(&flags, &stats, t0.elapsed())?;
            continue;
        }

        let mut buf = [0u8; 2048];
        socket.set_read_timeout(Some(Duration::from_millis(20)))?;
        match socket.recv_from(&mut buf) {
            Ok((n, src)) => {
                if let Some(pkt) = parse(&buf[..n]) {
                    if role == Role::Server && matches!(pkt.kind, PacketKind::Hello) {
                        let client_port = pkt.seq;
                        let client_addr: SocketAddr =
                            format!("{}:{client_port}", src.ip()).parse()?;
                        *peer.lock().unwrap() = Some(client_addr);
                        socket.send_to(&encode_simple(TYPE_HELLO_ACK), client_addr)?;
                        eprintln!("\rClient connected: {client_addr}");
                    }

                    handle_control(&pkt, &flags, &stats, &peer, &socket, role)?;

                    if matches!(pkt.kind, PacketKind::Resume) {
                        primed = false;
                        jitter_buf.clear();
                    }

                    if matches!(pkt.kind, PacketKind::Audio) {
                        received += 1;
                        if let Some(prev) = last_seq {
                            let diff = pkt.seq.wrapping_sub(prev) as i32;
                            if diff > 1 {
                                lost += (diff - 1) as u64;
                            }
                        }
                        last_seq = Some(pkt.seq);

                        let ia = last_arrival.elapsed().as_secs_f64() * 1000.0;
                        if jitter_n > 0 {
                            jitter_sum += (ia - FRAME_MS as f64).abs();
                        }
                        jitter_n += 1;
                        last_arrival = Instant::now();

                        if !flags.mute_remote.load(Ordering::Relaxed) {
                            let mut pcm = vec![0i16; crate::protocol::FRAME_SAMPLES];
                            let decoded = codec.lock().unwrap().decode(&pkt.payload, &mut pcm);
                            if let Ok(n) = decoded {
                                if n > 0 {
                                    jitter_buf.push(pcm[..n.min(pcm.len())].to_vec());
                                }
                                if !primed && jitter_buf.len() >= JITTER_BUFFER_FRAMES {
                                    primed = true;
                                }
                                if primed {
                                    if let Some(frame) = jitter_buf.first().cloned() {
                                        jitter_buf.remove(0);
                                        buffers.push_playback_frame(&frame);
                                    }
                                }
                            }
                        }

                        rx_bytes += n as u64;
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
        }

        if window.elapsed() >= Duration::from_secs(1) {
            let mut s = stats.lock().unwrap();
            s.rx_kbps = (rx_bytes as f64 * 8.0) / 1000.0;
            if received + lost > 0 {
                s.loss_pct = (lost as f64 / (received + lost) as f64) * 100.0;
            }
            if jitter_n > 1 {
                s.jitter_ms = jitter_sum / (jitter_n - 1) as f64;
            }
            jitter_sum = 0.0;
            jitter_n = 0;
            rx_bytes = 0;
            received = 0;
            lost = 0;
            window = Instant::now();
        }

        print_status(&flags, &stats, t0.elapsed())?;
    }

    if let Some(addr) = *peer.lock().unwrap() {
        let _ = socket.send_to(&encode_simple(TYPE_BYE), peer_dest(role, addr));
    }
    Ok(())
}

fn handle_control(
    pkt: &ParsedPacket,
    flags: &SessionFlags,
    stats: &Mutex<Stats>,
    peer: &Mutex<Option<SocketAddr>>,
    socket: &UdpSocket,
    role: Role,
) -> Result<(), Box<dyn std::error::Error>> {
    match pkt.kind {
        PacketKind::Ping => {
            if let Some(addr) = *peer.lock().unwrap() {
                let pong = encode_pong(pkt.timestamp);
                socket.send_to(&pong, peer_dest(role, addr))?;
            }
        }
        PacketKind::Pong => {
            stats.lock().unwrap().rtt_ms = now_ms().wrapping_sub(pkt.timestamp) as f64;
        }
        PacketKind::Bye => {
            flags.running.store(false, Ordering::Relaxed);
        }
        _ => {}
    }
    Ok(())
}

fn peer_dest(_role: Role, addr: SocketAddr) -> SocketAddr {
    addr
}

fn now_ms() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u32
}

fn print_controls() {
    println!();
    println!("Keys: m=mute mic  M=mute remote  p=pause/resume  q=quit");
    println!(
        "Audio: Opus mono {SAMPLE_RATE} Hz ~20 kbps/dir | ports {PORT_MAC_TO_SERVER}/{PORT_SERVER_TO_MAC}"
    );
    println!();
}

fn print_status(
    flags: &SessionFlags,
    stats: &Mutex<Stats>,
    elapsed: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let s = stats.lock().unwrap();
    let state = if flags.paused.load(Ordering::Relaxed) {
        "PAUSED"
    } else if flags.mute_mic.load(Ordering::Relaxed) && flags.mute_remote.load(Ordering::Relaxed) {
        "mic+remote muted"
    } else if flags.mute_mic.load(Ordering::Relaxed) {
        "mic muted"
    } else if flags.mute_remote.load(Ordering::Relaxed) {
        "remote muted"
    } else {
        "LIVE"
    };

    print!(
        "\r[{state}] {elapsed:.0?} | RTT {:.0} ms | jitter {:.1} ms | loss {:.1}% | up {:.1} kbps | down {:.1} kbps   ",
        s.rtt_ms, s.jitter_ms, s.loss_pct, s.tx_kbps, s.rx_kbps,
    );
    io::stdout().flush()?;
    Ok(())
}
