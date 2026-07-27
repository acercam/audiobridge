pub const PORT_MAC_TO_SERVER: u16 = 5004;
pub const PORT_SERVER_TO_MAC: u16 = 5005;

pub const SAMPLE_RATE: u32 = 24000;
pub const FRAME_MS: u32 = 20;
pub const FRAME_SAMPLES: usize = (SAMPLE_RATE as usize * FRAME_MS as usize) / 1000;
pub const OPUS_BITRATE: i32 = 20_000;
pub const JITTER_BUFFER_FRAMES: usize = 2;

pub const TYPE_AUDIO: u8 = 0x01;
pub const TYPE_HELLO: u8 = 0x10;
pub const TYPE_HELLO_ACK: u8 = 0x11;
pub const TYPE_RESUME: u8 = 0x12;
pub const TYPE_BYE: u8 = 0x13;
pub const TYPE_PING: u8 = 0x14;
pub const TYPE_PONG: u8 = 0x15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketKind {
    Audio,
    Hello,
    HelloAck,
    Resume,
    Bye,
    Ping,
    Pong,
    Unknown(u8),
}

impl From<u8> for PacketKind {
    fn from(v: u8) -> Self {
        match v {
            TYPE_AUDIO => Self::Audio,
            TYPE_HELLO => Self::Hello,
            TYPE_HELLO_ACK => Self::HelloAck,
            TYPE_RESUME => Self::Resume,
            TYPE_BYE => Self::Bye,
            TYPE_PING => Self::Ping,
            TYPE_PONG => Self::Pong,
            other => Self::Unknown(other),
        }
    }
}

pub fn encode_audio(seq: u16, timestamp: u32, opus: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + 2 + 4 + opus.len());
    buf.push(TYPE_AUDIO);
    buf.extend_from_slice(&seq.to_be_bytes());
    buf.extend_from_slice(&timestamp.to_be_bytes());
    buf.extend_from_slice(opus);
    buf
}

pub fn encode_hello(client_port: u16) -> Vec<u8> {
    let mut buf = vec![TYPE_HELLO];
    buf.extend_from_slice(&client_port.to_be_bytes());
    buf
}

pub fn encode_simple(kind: u8) -> Vec<u8> {
    vec![kind]
}

pub fn encode_ping(timestamp: u32) -> Vec<u8> {
    let mut buf = vec![TYPE_PING];
    buf.extend_from_slice(&timestamp.to_be_bytes());
    buf
}

pub fn encode_pong(timestamp: u32) -> Vec<u8> {
    let mut buf = vec![TYPE_PONG];
    buf.extend_from_slice(&timestamp.to_be_bytes());
    buf
}

pub struct ParsedPacket {
    pub kind: PacketKind,
    pub seq: u16,
    pub timestamp: u32,
    pub payload: Vec<u8>,
}

pub fn parse(data: &[u8]) -> Option<ParsedPacket> {
    if data.is_empty() {
        return None;
    }
    let kind = PacketKind::from(data[0]);
    match kind {
        PacketKind::Audio if data.len() >= 7 => {
            let seq = u16::from_be_bytes([data[1], data[2]]);
            let timestamp = u32::from_be_bytes([data[3], data[4], data[5], data[6]]);
            Some(ParsedPacket {
                kind,
                seq,
                timestamp,
                payload: data[7..].to_vec(),
            })
        }
        PacketKind::Hello if data.len() >= 3 => {
            let port = u16::from_be_bytes([data[1], data[2]]);
            Some(ParsedPacket {
                kind,
                seq: port,
                timestamp: 0,
                payload: Vec::new(),
            })
        }
        PacketKind::Ping | PacketKind::Pong if data.len() >= 5 => {
            let timestamp = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
            Some(ParsedPacket {
                kind,
                seq: 0,
                timestamp,
                payload: Vec::new(),
            })
        }
        PacketKind::HelloAck | PacketKind::Resume | PacketKind::Bye => Some(ParsedPacket {
            kind,
            seq: 0,
            timestamp: 0,
            payload: Vec::new(),
        }),
        _ => None,
    }
}
