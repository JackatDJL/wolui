import { useEffect, useRef, useState } from 'react'
import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'
import { RefreshCw, X } from 'lucide-react'

type Device = {
  id: string
  name: string
  tailscale_name: string | null
  ssh_username: string | null
}

export default function SshTerminalDialog({ siteId, device, onClose }: { siteId: string; device: Device; onClose: () => void }) {
  const terminalHost = useRef<HTMLDivElement>(null)
  const [attempt, setAttempt] = useState(0)
  const [status, setStatus] = useState<'connecting' | 'connected' | 'closed' | 'error'>('connecting')
  const [message, setMessage] = useState('')

  useEffect(() => {
    const host = terminalHost.current
    if (!host) return

    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
      fontSize: 13,
      convertEol: true,
      theme: { background: '#111915', foreground: '#e7eee9', cursor: '#7dc99a' },
    })
    const fit = new FitAddon()
    terminal.loadAddon(fit)
    terminal.open(host)

    const scheme = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    const path = `/api/v1/sites/${encodeURIComponent(siteId)}/devices/${encodeURIComponent(device.id)}/ssh`
    const socket = new WebSocket(`${scheme}//${window.location.host}${path}`)
    socket.binaryType = 'arraybuffer'
    let authorized = false

    const sendResize = () => {
      try { fit.fit() } catch { /* terminal is not laid out yet */ }
      if (authorized && socket.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify({ type: 'resize', cols: terminal.cols, rows: terminal.rows }))
      }
    }
    const observer = new ResizeObserver(sendResize)
    observer.observe(host)
    window.addEventListener('resize', sendResize)

    const dataSubscription = terminal.onData(data => {
      if (authorized && socket.readyState === WebSocket.OPEN) socket.send(new TextEncoder().encode(data))
    })

    socket.onmessage = event => {
      if (typeof event.data !== 'string') {
        if (event.data instanceof ArrayBuffer) terminal.write(new Uint8Array(event.data))
        else if (event.data instanceof Blob) void event.data.arrayBuffer().then(data => terminal.write(new Uint8Array(data)))
        return
      }
      try {
        const payload = JSON.parse(event.data) as { type?: string; session_id?: string; message?: string }
        if (payload.type === 'ready') {
          authorized = true
          setStatus('connected')
          setMessage('')
          requestAnimationFrame(() => { sendResize(); terminal.focus() })
        } else if (payload.type === 'error') {
          authorized = false
          setStatus('error')
          setMessage(payload.message || 'Die SSH-Verbindung wurde abgelehnt.')
        }
      } catch {
        authorized = false
        setStatus('error')
        setMessage('Der Server hat eine ungültige SSH-Antwort gesendet.')
      }
    }
    socket.onerror = () => {
      if (!authorized) {
        setStatus('error')
        setMessage('Die SSH-Verbindung konnte nicht aufgebaut werden. Prüfe die Serverfreigabe und Tailscale-Verbindung.')
      }
    }
    socket.onclose = () => {
      if (authorized) {
        authorized = false
        setStatus('closed')
        setMessage('Die SSH-Verbindung wurde geschlossen. Du kannst sie manuell erneut starten.')
      } else {
        setStatus(current => current === 'error' ? current : 'error')
        setMessage(current => current || 'Der Server hat die SSH-Verbindung beendet. Du kannst sie manuell erneut starten.')
      }
    }

    return () => {
      authorized = false
      observer.disconnect()
      window.removeEventListener('resize', sendResize)
      dataSubscription.dispose()
      socket.close(1000, 'terminal closed')
      terminal.dispose()
    }
  }, [attempt, device.id, siteId])

  const reconnect = () => { setMessage(''); setStatus('connecting'); setAttempt(value => value + 1) }
  return <div className="modal-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) onClose() }}>
    <section className="dialog ssh-dialog" role="dialog" aria-modal="true" aria-labelledby="ssh-title">
      <div className="dialog-head"><div><div className="eyebrow">TAILSCALE SSH</div><h2 id="ssh-title">{device.name}</h2></div><button className="icon-button" onClick={onClose} aria-label="Terminal schließen"><X size={18} /></button></div>
      <p className="muted">Verbindung zu <code>{device.ssh_username || 'Systembenutzer'}@{device.tailscale_name}</code>. Die Anmeldung und Hostfreigabe verwaltet dein Tailnet.</p>
      <div className="ssh-status" role="status"><i className={`ssh-status-dot ${status}`} />{status === 'connecting' ? 'SSH-Verbindung wird aufgebaut …' : status === 'connected' ? 'Verbunden' : status === 'closed' ? 'Verbindung geschlossen' : 'Verbindung fehlgeschlagen'}</div>
      <div className="ssh-terminal" ref={terminalHost} aria-label="SSH-Terminal" />
      {message && <p className="ssh-error" role="alert">{message}</p>}
      <div className="dialog-actions">{status !== 'connected' && <button className="secondary" onClick={reconnect}><RefreshCw size={14} /> Neu verbinden</button>}<button className="primary" onClick={onClose}>Schließen</button></div>
    </section>
  </div>
}
