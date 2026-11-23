import { FormEvent, useEffect, useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Activity, ArrowRight, Check, ChevronDown, CircleHelp, Computer, Cpu, LoaderCircle, LogOut, Plus, Power, RefreshCw, Settings2, ShieldCheck, Trash2, X } from 'lucide-react'

type ApiError = { code: string; message: string }
type Site = { id: string; name: string }
type Profile = { id: string; name: string }
type Device = { id: string; name: string; mac: string; network_profile_id: string; tailscale_name: string | null; revision: number }
type WakeJob = { id: string; status: string }

class RequestError extends Error {
  code: string
  constructor(error: ApiError, status: number) { super(error.message || `Anfrage fehlgeschlagen (${status})`); this.code = error.code }
}

let csrfToken = ''
async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers)
  if (init.body) headers.set('Content-Type', 'application/json')
  if (['POST', 'PATCH', 'DELETE'].includes(init.method || 'GET') && csrfToken) headers.set('X-CSRF-Token', csrfToken)
  const response = await fetch(path, { ...init, headers, credentials: 'same-origin' })
  if (!response.ok) {
    let body: ApiError = { code: 'request_failed', message: 'Die Anfrage konnte nicht abgeschlossen werden.' }
    try { body = await response.json() as ApiError } catch { /* use fallback */ }
    throw new RequestError(body, response.status)
  }
  if (response.status === 204) return undefined as T
  return await response.json() as T
}

function useSession() {
  return useQuery({ queryKey: ['session'], queryFn: async () => {
    const result = await request<{ csrf_token: string }>('/api/v1/sessions/current')
    csrfToken = result.csrf_token
    return result
  }, retry: false })
}

function App() {
  const queryClient = useQueryClient()
  const session = useSession()
  const [token, setToken] = useState('')
  const [loginError, setLoginError] = useState('')
  const [selectedSite, setSelectedSite] = useState('')
  const [editing, setEditing] = useState<Device | 'new' | null>(null)
  const [notice, setNotice] = useState('')
  const [jobs, setJobs] = useState<Record<string, WakeJob>>({})
  const [visibilityTick, setVisibilityTick] = useState(0)

  useEffect(() => {
    const onVisibilityChange = () => setVisibilityTick(value => value + 1)
    document.addEventListener('visibilitychange', onVisibilityChange)
    return () => document.removeEventListener('visibilitychange', onVisibilityChange)
  }, [])
  useEffect(() => { setJobs({}) }, [selectedSite])

  const login = useMutation({ mutationFn: () => request<{ csrf_token: string }>('/api/v1/sessions', { method: 'POST', body: JSON.stringify({ token }) }), onSuccess: result => { csrfToken = result.csrf_token; setToken(''); setLoginError(''); void queryClient.invalidateQueries({ queryKey: ['session'] }) }, onError: e => setLoginError(errorText(e)) })
  const logout = useMutation({ mutationFn: () => request<void>('/api/v1/sessions/current', { method: 'DELETE' }), onSuccess: () => { csrfToken = ''; queryClient.clear() } })
  const authenticated = session.isSuccess
  const sites = useQuery({ queryKey: ['sites'], queryFn: () => request<Site[]>('/api/v1/sites'), enabled: authenticated })
  useEffect(() => { if (sites.data?.length && (!selectedSite || !sites.data.some(s => s.id === selectedSite))) setSelectedSite(sites.data[0].id) }, [sites.data, selectedSite])
  const site = sites.data?.find(s => s.id === selectedSite)
  const profiles = useQuery({ queryKey: ['profiles', selectedSite], queryFn: () => request<Profile[]>(`/api/v1/sites/${encodeURIComponent(selectedSite)}/network-profiles`), enabled: authenticated && !!selectedSite })
  const devices = useQuery({ queryKey: ['devices', selectedSite], queryFn: () => request<Device[]>(`/api/v1/sites/${encodeURIComponent(selectedSite)}/devices`), enabled: authenticated && !!selectedSite })
  const sortedDevices = useMemo(() => [...(devices.data ?? [])].sort((a, b) => a.name.localeCompare(b.name)), [devices.data])

  const wake = useMutation({ mutationFn: async (device: Device) => {
    const response = await request<WakeJob>(`/api/v1/sites/${encodeURIComponent(selectedSite)}/devices/${encodeURIComponent(device.id)}/wake`, { method: 'POST', headers: { 'Idempotency-Key': crypto.randomUUID() }, body: '{}' })
    setJobs(current => ({ ...current, [device.id]: response }))
    return response
  }, onError: e => setNotice(errorText(e)) })
  const saveDevice = useMutation({ mutationFn: async (value: Omit<Device, 'id' | 'revision'> & { id?: string; revision?: number }) => {
    const path = `/api/v1/sites/${encodeURIComponent(selectedSite)}/devices${value.id ? `/${encodeURIComponent(value.id)}` : ''}`
    return request<Device>(path, { method: value.id ? 'PATCH' : 'POST', body: JSON.stringify(value.id ? { name: value.name, mac: value.mac, network_profile_id: value.network_profile_id, tailscale_name: value.tailscale_name || null, revision: value.revision } : { name: value.name, mac: value.mac, network_profile_id: value.network_profile_id, tailscale_name: value.tailscale_name || null }) })
  }, onSuccess: () => { setEditing(null); void queryClient.invalidateQueries({ queryKey: ['devices', selectedSite] }); setNotice('Gerät gespeichert.') }, onError: e => setNotice(errorText(e)) })
  const removeDevice = useMutation({ mutationFn: (id: string) => request<void>(`/api/v1/sites/${encodeURIComponent(selectedSite)}/devices/${encodeURIComponent(id)}`, { method: 'DELETE' }), onSuccess: () => { void queryClient.invalidateQueries({ queryKey: ['devices', selectedSite] }); setNotice('Gerät entfernt.') }, onError: e => setNotice(errorText(e)) })

  useEffect(() => {
    const pending = Object.entries(jobs).filter(([, job]) => ['queued', 'sending', 'pending', 'accepted'].includes(job.status))
    if (!pending.length || document.hidden) return
    const timer = window.setTimeout(async () => {
      const updates = await Promise.allSettled(pending.map(async ([deviceId, job]) => {
        const next = await request<WakeJob>(`/api/v1/sites/${encodeURIComponent(selectedSite)}/wake-jobs/${encodeURIComponent(job.id)}`)
        return [deviceId, next] as const
      }))
      setJobs(current => { const updated = { ...current }; for (const item of updates) if (item.status === 'fulfilled') updated[item.value[0]] = item.value[1]; return updated })
    }, 1800)
    return () => window.clearTimeout(timer)
  }, [jobs, selectedSite, visibilityTick])

  if (session.isPending) return <div className="boot"><LoaderCircle className="spin" /> Verbindung wird geprüft</div>
  if (!authenticated) return <main className="login-page"><div className="login-card"><Brand /><div className="eyebrow">ADMIN ZUGANG</div><h1>Willkommen zurück.</h1><p className="muted">Melde dich an, um deine Geräte sicher zu verwalten.</p><form onSubmit={(event: FormEvent) => { event.preventDefault(); login.mutate() }}><label htmlFor="token">Zugangstoken</label><input id="token" type="password" autoComplete="current-password" placeholder="Token einfügen" value={token} onChange={e => setToken(e.target.value)} required /><button className="primary full" disabled={login.isPending || !token}>{login.isPending ? <LoaderCircle className="spin" size={17} /> : null} Verbinden <ArrowRight size={17} /></button>{loginError && <p className="form-error" role="alert">{loginError}</p>}</form><div className="secure-note"><ShieldCheck size={16} /> Geschützte Verbindung</div></div><span className="login-footer">WOL UI <span>·</span> Wake on LAN, ganz einfach.</span></main>

  return <div className="app-shell">
    <aside className="sidebar"><Brand /><div className="nav-label">WORKSPACE</div><button className="nav-item active"><Computer size={18} /> Geräte <span className="nav-count">{devices.data?.length ?? '—'}</span></button><button className="nav-item" onClick={() => setNotice('Standortverwaltung wird mit der Standortkopplung verfügbar.')}><Activity size={18} /> Standorte</button><div className="sidebar-bottom"><div className="help-box"><div className="help-icon"><CircleHelp size={18} /></div><strong>Brauchst du Hilfe?</strong><span>Tipps für Wake-on-LAN</span><a href="https://tailscale.com/kb/" target="_blank" rel="noreferrer">Mehr erfahren <ArrowRight size={13} /></a></div><button className="account" onClick={() => logout.mutate()}><div className="avatar">W</div><div><strong>Administrator</strong><span>Lokale Sitzung</span></div><LogOut size={17} /></button></div></aside>
    <main className="main-content"><header className="topbar"><div className="breadcrumb">Workspace <span>/</span> <strong>Geräte</strong></div><div className="top-actions"><span className="online"><i /> System aktiv</span><button className="icon-button" aria-label="Aktualisieren" onClick={() => { void queryClient.invalidateQueries({ queryKey: ['devices', selectedSite] }); void queryClient.invalidateQueries({ queryKey: ['sites'] }) }}><RefreshCw size={17} /></button></div></header>
      <div className="content-wrap"><div className="page-heading"><div><div className="eyebrow">DEIN NETZWERK</div><h1>Geräte</h1><p>Verwalte und starte deine Geräte an einem Ort.</p></div><button className="primary" onClick={() => setEditing('new')} disabled={!selectedSite}><Plus size={17} /> Gerät hinzufügen</button></div>
        <section className="overview"><div className="overview-copy"><div className="overview-icon"><Power size={19} /></div><div><span className="small-label">ÜBERSICHT</span><h2>Dein Netzwerk auf einen Blick</h2><p>Geräte per Wake-on-LAN sicher starten.</p></div></div><div className="stats"><div><strong>{devices.data?.length ?? '—'}</strong><span>Geräte</span></div><div className="stat-divider" /><div><strong>{site ? '1' : '—'}</strong><span>Standort</span></div><div className="stat-divider" /><div className="status-stat"><i /><strong>Verbunden</strong><span>Status</span></div></div></section>
        <section className="device-section"><div className="section-heading"><div><h2>Alle Geräte <span className="count-pill">{devices.data?.length ?? 0}</span></h2><p>Wähle ein Gerät, um es zu starten oder zu bearbeiten.</p></div><div className="site-select-wrap"><Settings2 size={15} /><select aria-label="Standort wählen" value={selectedSite} onChange={e => setSelectedSite(e.target.value)} disabled={!sites.data?.length}>{sites.data?.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select><ChevronDown size={15} /></div></div>
          {sites.isLoading || devices.isLoading ? <div className="loading-state"><LoaderCircle className="spin" /> Geräte werden geladen</div> : sites.isError || devices.isError ? <div className="empty-state"><div className="empty-icon"><Activity /></div><h3>Geräte konnten nicht geladen werden</h3><p>{errorText(sites.error ?? devices.error)}</p><button className="secondary" onClick={() => { void sites.refetch(); void devices.refetch() }}>Erneut versuchen</button></div> : !sites.data?.length ? <div className="empty-state"><div className="empty-icon"><Computer /></div><h3>Noch kein Standort vorhanden</h3><p>Dein lokaler Standort wird hier angezeigt, sobald er eingerichtet ist.</p></div> : sortedDevices.length === 0 ? <div className="empty-state"><div className="empty-icon"><Computer /></div><h3>Deine Geräteliste ist noch leer</h3><p>Füge dein erstes Gerät hinzu, um es mit einem Klick zu starten.</p><button className="primary" onClick={() => setEditing('new')}><Plus size={16} /> Gerät hinzufügen</button></div> : <div className="device-list">{sortedDevices.map(device => <DeviceRow key={device.id} device={device} profile={profiles.data?.find(p => p.id === device.network_profile_id)} job={jobs[device.id]} waking={wake.isPending && wake.variables?.id === device.id} onWake={() => wake.mutate(device)} onEdit={() => setEditing(device)} onDelete={() => { if (window.confirm(`„${device.name}“ wirklich entfernen?`)) removeDevice.mutate(device.id) }} />)}</div>}
        </section><footer className="page-footer"><span>WOL UI <span className="footer-dot">·</span> Lokal und sicher.</span><span><i /> Alle Systeme erreichbar</span></footer>
      </div>
    </main>
    {editing && <DeviceDialog value={editing === 'new' ? null : editing} profiles={profiles.data ?? []} onClose={() => setEditing(null)} onSave={value => saveDevice.mutate(value)} saving={saveDevice.isPending} />}
    {notice && <div className="toast" role="status">{notice}<button onClick={() => setNotice('')} aria-label="Meldung schließen"><X size={16} /></button></div>}
  </div>
}

function DeviceRow({ device, profile, job, waking, onWake, onEdit, onDelete }: { device: Device; profile?: Profile; job?: WakeJob; waking: boolean; onWake: () => void; onEdit: () => void; onDelete: () => void }) {
  const status = job?.status
  const sent = status === 'sent'
  const failed = status === 'failed'
  const unknown = status === 'unknown'
  const pending = waking || ['queued', 'sending', 'pending', 'accepted'].includes(status ?? '')
  return <article className="device-row"><div className="device-mark"><Computer size={21} /></div><div className="device-info"><div className="device-name-line"><h3>{device.name}</h3>{sent ? <span className="result sent"><Check size={12} /> Paket gesendet</span> : failed ? <span className="result failed">Versand fehlgeschlagen</span> : unknown ? <span className="result unknown">Versand unklar</span> : null}</div><div className="device-meta"><span><Cpu size={13} /> {device.mac}</span><i /> <span>{profile?.name ?? 'Netzwerkprofil'}</span>{device.tailscale_name && <><i /><span className="tailnet">{device.tailscale_name}</span></>}</div></div><div className="row-actions"><button className="text-action" onClick={onEdit}>Bearbeiten</button><button className="icon-button subtle" onClick={onDelete} aria-label={`${device.name} entfernen`}><Trash2 size={16} /></button><button className="wake-button" onClick={onWake} disabled={pending} aria-label={`${device.name} starten`}>{pending ? <LoaderCircle size={16} className="spin" /> : <Power size={16} />}{pending ? 'Wird gesendet' : 'Starten'}</button></div></article>
}

function DeviceDialog({ value, profiles, onClose, onSave, saving }: { value: Device | null; profiles: Profile[]; onClose: () => void; onSave: (device: Omit<Device, 'id' | 'revision'> & { id?: string; revision?: number }) => void; saving: boolean }) {
  const [name, setName] = useState(value?.name ?? '')
  const [mac, setMac] = useState(value?.mac ?? '')
  const [profile, setProfile] = useState(value?.network_profile_id ?? profiles[0]?.id ?? '')
  const [tailscale, setTailscale] = useState(value?.tailscale_name ?? '')
  return <div className="modal-backdrop" onMouseDown={e => { if (e.target === e.currentTarget) onClose() }}><section className="dialog" role="dialog" aria-modal="true" aria-labelledby="dialog-title"><div className="dialog-head"><div><div className="eyebrow">GERÄTEINVENTAR</div><h2 id="dialog-title">{value ? 'Gerät bearbeiten' : 'Gerät hinzufügen'}</h2></div><button className="icon-button" onClick={onClose} aria-label="Schließen"><X size={18} /></button></div><form onSubmit={e => { e.preventDefault(); onSave({ ...(value ? { id: value.id, revision: value.revision } : {}), name: name.trim(), mac: mac.trim(), network_profile_id: profile, tailscale_name: tailscale.trim() }) }}><label htmlFor="device-name">Gerätename</label><input id="device-name" autoFocus value={name} onChange={e => setName(e.target.value)} placeholder="z. B. Arbeits-PC" required maxLength={80} /><label htmlFor="device-mac">MAC-Adresse</label><input id="device-mac" value={mac} onChange={e => setMac(e.target.value)} placeholder="AA:BB:CC:DD:EE:FF" required pattern="([0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2}" /><label htmlFor="network-profile">Netzwerkprofil</label><select id="network-profile" value={profile} onChange={e => setProfile(e.target.value)} required disabled={!profiles.length}><option value="" disabled>Profil auswählen</option>{profiles.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select>{profiles.length === 0 && <p className="field-hint">Für diesen Standort ist kein Netzwerkprofil verfügbar.</p>}<label htmlFor="tailscale-name">Tailscale-Name <span className="optional">OPTIONAL</span></label><input id="tailscale-name" value={tailscale} onChange={e => setTailscale(e.target.value)} placeholder="pc.tailnet.ts.net" /><div className="dialog-actions"><button type="button" className="secondary" onClick={onClose}>Abbrechen</button><button className="primary" disabled={saving || !profiles.length}>{saving && <LoaderCircle size={16} className="spin" />}{value ? 'Änderungen speichern' : 'Gerät hinzufügen'}</button></div></form></section></div>
}

function Brand() { return <div className="brand"><div className="brand-mark"><Power size={20} strokeWidth={2.4} /></div><span>WOL<span className="brand-light">UI</span></span></div> }
function errorText(error: unknown) { return error instanceof Error ? error.message : 'Ein unerwarteter Fehler ist aufgetreten.' }

export default App
