# audiobridge

Puente de audio bidireccional de baja latencia sobre UDP, pensado para redes Tailscale.

- **Codec:** Opus mono 24 kHz (~20 kbps por dirección)
- **Puertos:** `5004` (Mac → servidor), `5005` (servidor → Mac)
- **Latencia objetivo:** ~70–130 ms (jitter buffer 40 ms + red)

## Instalación

```bash
cargo install --git https://github.com/acercam/audiobridge.git
```

En Linux, instala dependencias de audio primero:

```bash
sudo apt install libopus-dev libasound2-dev pkg-config
```

## Uso

**Servidor Linux** (antes de conectar desde la Mac):

```bash
audiobridge listen
# USB Audio Device se selecciona automáticamente en Linux
# audiobridge listen --device "USB Audio Device"
```

**Mac** (inicia la sesión):

```bash
audiobridge connect 100.64.0.33
```

**Durante la sesión:**

| Tecla | Acción |
|-------|--------|
| `m` | Mute micrófono local (deja de enviar) |
| `M` | Mute remoto (no reproduce audio entrante) |
| `p` | Pausa / reanuda (**cero tráfico** en pausa) |
| `q` | Salir |

**Utilidades:**

```bash
audiobridge devices   # listar dispositivos de audio
```

## systemd (opcional, servidor)

```ini
[Unit]
Description=audiobridge listener
After=network-online.target tailscaled.service

[Service]
ExecStart=/usr/local/bin/audiobridge listen
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

## Notas

- Sin cancelación de eco: usa volumen moderado en altavoz o auriculares si hay acoplamiento.
- Tailscale cifra el tráfico; no se usa TLS adicional.
- La pausa detiene captura, envío, recepción y reproducción por completo.
