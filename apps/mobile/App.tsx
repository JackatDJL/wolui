import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  ActivityIndicator,
  Linking,
  Pressable,
  RefreshControl,
  SafeAreaView,
  ScrollView,
  StatusBar,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native'
import * as SecureStore from 'expo-secure-store'

type Credentials = { baseUrl: string; sessionToken: string; csrfToken: string }
type ApiError = { code?: string; message?: string }
type Site = { id: string; name: string; local: boolean; online: boolean; last_seen_at: number | null }
type Profile = { id: string; name: string }
type Device = {
  id: string
  name: string
  mac: string
  network_profile_id: string
  tailscale_name: string | null
  ssh_enabled: boolean
  ssh_username: string | null
  revision: number
}
type WakeJob = { id: string; status: string }

const CREDENTIALS_KEY = 'wolui.mobile.session.v1'
const DEFAULT_API_URL = process.env.EXPO_PUBLIC_API_URL ?? ''

async function api<T>(credentials: Credentials, path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers)
  headers.set('Authorization', 'Bearer ' + credentials.sessionToken)
  if (init.body) headers.set('Content-Type', 'application/json')
  if (['POST', 'PATCH', 'DELETE'].includes(init.method ?? 'GET')) {
    headers.set('X-CSRF-Token', credentials.csrfToken)
  }

  const response = await fetch(credentials.baseUrl + path, { ...init, headers })
  if (!response.ok) {
    let body: ApiError = {}
    try { body = await response.json() as ApiError } catch { /* response may have no JSON body */ }
    throw new Error(body.message || 'Anfrage fehlgeschlagen (' + response.status + ')')
  }
  if (response.status === 204) return undefined as T
  return await response.json() as T
}

function cleanBaseUrl(value: string): string {
  const parsed = new URL(value.trim())
  const isLoopback = ['localhost', '127.0.0.1', '10.0.2.2'].includes(parsed.hostname)
  if (parsed.protocol !== 'https:' && !(isLoopback && parsed.protocol === 'http:') && !(__DEV__ && parsed.protocol === 'http:')) {
    throw new Error('Bitte verwende eine HTTPS-Adresse für den Server.')
  }
  if (parsed.username || parsed.password || parsed.search || parsed.hash) {
    throw new Error('Die Server-Adresse darf keine Zugangsdaten oder URL-Parameter enthalten.')
  }
  return parsed.toString().replace(/\/+$/, '')
}

function App() {
  const [credentials, setCredentials] = useState<Credentials | null>(null)
  const [baseUrl, setBaseUrl] = useState(DEFAULT_API_URL)
  const [accessToken, setAccessToken] = useState('')
  const [booting, setBooting] = useState(true)
  const [connecting, setConnecting] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState('')
  const [sites, setSites] = useState<Site[]>([])
  const [profiles, setProfiles] = useState<Profile[]>([])
  const [devices, setDevices] = useState<Device[]>([])
  const [selectedSite, setSelectedSite] = useState('')
  const [jobs, setJobs] = useState<Record<string, WakeJob>>({})
  const [waking, setWaking] = useState<string | null>(null)

  const selectedSiteName = sites.find(site => site.id === selectedSite)?.name ?? 'Standort'
  const selectedSiteOnline = sites.find(site => site.id === selectedSite)?.online ?? false
  const sortedDevices = useMemo(() => [...devices].sort((a, b) => a.name.localeCompare(b.name)), [devices])

  const clearSession = useCallback(async () => {
    await SecureStore.deleteItemAsync(CREDENTIALS_KEY)
    setCredentials(null)
    setSites([])
    setProfiles([])
    setDevices([])
    setJobs({})
  }, [])

  const refresh = useCallback(async (auth = credentials, siteId = selectedSite) => {
    if (!auth) return
    setRefreshing(true)
    setError('')
    try {
      const nextSites = await api<Site[]>(auth, '/api/v1/sites')
      setSites(nextSites)
      const currentSite = nextSites.some(site => site.id === siteId) ? siteId : nextSites[0]?.id ?? ''
      setSelectedSite(currentSite)
      if (!currentSite) {
        setDevices([])
        setProfiles([])
        return
      }
      const sitePath = '/api/v1/sites/' + encodeURIComponent(currentSite)
      const [nextDevices, nextProfiles] = await Promise.all([
        api<Device[]>(auth, sitePath + '/devices'),
        api<Profile[]>(auth, sitePath + '/network-profiles'),
      ])
      setDevices(nextDevices)
      setProfiles(nextProfiles)
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : 'Die Verbindung zum Server ist fehlgeschlagen.'
      setError(message)
      if (/session|authentifiz|unauthorized/i.test(message)) await clearSession()
    } finally {
      setRefreshing(false)
    }
  }, [clearSession, credentials, selectedSite])

  useEffect(() => {
    let mounted = true
    void SecureStore.getItemAsync(CREDENTIALS_KEY)
      .then(saved => {
        if (!mounted || !saved) return
        const restored = JSON.parse(saved) as Credentials
        setCredentials(restored)
        setBaseUrl(restored.baseUrl)
      })
      .catch(() => { if (mounted) setError('Die gespeicherte Sitzung konnte nicht gelesen werden.') })
      .finally(() => { if (mounted) setBooting(false) })
    return () => { mounted = false }
  }, [])

  useEffect(() => {
    if (credentials) void refresh(credentials, selectedSite)
  }, [credentials, selectedSite, refresh])

  useEffect(() => { setJobs({}) }, [selectedSite])

  useEffect(() => {
    const pending = Object.entries(jobs).filter(([, job]) => ['queued', 'sending', 'pending', 'accepted'].includes(job.status))
    if (!credentials || !selectedSite || pending.length === 0) return
    const timer = setTimeout(() => {
      void Promise.all(pending.map(async ([deviceId, job]) => {
        try {
          const next = await api<WakeJob>(credentials, '/api/v1/sites/' + encodeURIComponent(selectedSite) + '/wake-jobs/' + encodeURIComponent(job.id))
          setJobs(current => ({ ...current, [deviceId]: next }))
        } catch { /* status remains visible and refresh can retry */ }
      }))
    }, 1400)
    return () => clearTimeout(timer)
  }, [credentials, jobs, selectedSite])

  async function signIn() {
    setConnecting(true)
    setError('')
    try {
      const origin = cleanBaseUrl(baseUrl)
      const response = await fetch(origin + '/api/v1/sessions', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token: accessToken.trim() }),
      })
      if (!response.ok) {
        let body: ApiError = {}
        try { body = await response.json() as ApiError } catch { /* use fallback */ }
        throw new Error(body.message || 'Anmeldung fehlgeschlagen (' + response.status + ')')
      }
      const result = await response.json() as { csrf_token?: string; session_token?: string }
      if (!result.session_token || !result.csrf_token) {
        throw new Error('Der Server muss Mobile-Sitzungstoken ausgeben. Bitte aktualisiere deine WOL UI Instanz.')
      }
      const next: Credentials = { baseUrl: origin, sessionToken: result.session_token, csrfToken: result.csrf_token }
      await SecureStore.setItemAsync(CREDENTIALS_KEY, JSON.stringify(next))
      setBaseUrl(origin)
      setCredentials(next)
      setAccessToken('')
      setSelectedSite('')
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Anmeldung fehlgeschlagen.')
    } finally {
      setConnecting(false)
    }
  }

  async function wakeDevice(device: Device) {
    if (!credentials || !selectedSite) return
    setWaking(device.id)
    setError('')
    try {
      const job = await api<WakeJob>(credentials, '/api/v1/sites/' + encodeURIComponent(selectedSite) + '/devices/' + encodeURIComponent(device.id) + '/wake', {
        method: 'POST',
        headers: { 'Idempotency-Key': Date.now() + '-' + Math.random().toString(36).slice(2) },
        body: JSON.stringify({}),
      })
      setJobs(current => ({ ...current, [device.id]: job }))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Das Startsignal konnte nicht gesendet werden.')
    } finally {
      setWaking(null)
    }
  }

  async function signOut() {
    if (credentials) {
      try { await api<void>(credentials, '/api/v1/sessions/current', { method: 'DELETE' }) } catch { /* local credentials are cleared either way */ }
    }
    await clearSession()
  }

  if (booting) return <Centered><ActivityIndicator color="#23744c" /><Text style={styles.bootText}>WOL UI wird geöffnet</Text></Centered>

  if (!credentials) return <SafeAreaView style={styles.safe}>
    <StatusBar barStyle="dark-content" />
    <ScrollView contentContainerStyle={styles.loginScroll} keyboardShouldPersistTaps="handled">
      <View style={styles.loginCard}>
        <Brand />
        <Text style={styles.eyebrow}>MOBILER ZUGANG</Text>
        <Text style={styles.loginTitle}>Dein Netzwerk,{"\n"}immer griffbereit.</Text>
        <Text style={styles.description}>Verbinde dich mit deiner WOL UI Instanz. Dein Sitzungstoken wird verschlüsselt auf diesem Gerät gespeichert.</Text>
        <Text style={styles.label}>Server-Adresse</Text>
        <TextInput autoCapitalize="none" autoCorrect={false} keyboardType="url" placeholder="https://wolui.example.com" value={baseUrl} onChangeText={setBaseUrl} style={styles.input} textContentType="URL" />
        <Text style={styles.label}>Zugangstoken</Text>
        <TextInput autoCapitalize="none" autoCorrect={false} placeholder="Token aus deiner Server-Konfiguration" value={accessToken} onChangeText={setAccessToken} style={styles.input} secureTextEntry textContentType="password" />
        {error ? <Text style={styles.error}>{error}</Text> : null}
        <Pressable onPress={() => void signIn()} disabled={connecting || !accessToken.trim() || !baseUrl.trim()} style={({ pressed }) => [styles.primaryButton, (pressed || connecting) && styles.buttonPressed, (!accessToken.trim() || !baseUrl.trim()) && styles.disabledButton]}>
          {connecting ? <ActivityIndicator color="#fff" /> : <Text style={styles.primaryButtonText}>Sicher verbinden  →</Text>}
        </Pressable>
        <Text style={styles.loginFoot}>Dein Zugangstoken wird nur zum Anmelden verwendet.</Text>
      </View>
    </ScrollView>
  </SafeAreaView>

  return <SafeAreaView style={styles.safe}>
    <StatusBar barStyle="dark-content" />
    <View style={styles.header}>
      <Brand />
      <Pressable accessibilityRole="button" onPress={() => void signOut()} style={styles.signOut}><Text style={styles.signOutText}>Abmelden</Text></Pressable>
    </View>
    <ScrollView contentContainerStyle={styles.content} refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => void refresh()} tintColor="#23744c" />}>
      <View style={styles.headingRow}>
        <View><Text style={styles.eyebrow}>DEIN NETZWERK</Text><Text style={styles.title}>Geräte</Text></View>
        <Pressable accessibilityRole="button" onPress={() => void refresh()} style={styles.refreshButton}><Text style={styles.refreshText}>↻  Aktualisieren</Text></Pressable>
      </View>
      <View style={styles.summary}>
        <View style={styles.summaryIcon}><Text style={styles.powerGlyph}>⏻</Text></View>
        <View style={styles.summaryCopy}><Text style={styles.summaryTitle}>{selectedSiteName}</Text><Text style={styles.summaryHint}>{devices.length} {devices.length === 1 ? 'Gerät' : 'Geräte'} bereit zum Starten</Text></View>
        <View style={[styles.liveDot, !selectedSiteOnline && styles.offlineDot]} />
      </View>
      {sites.length > 1 ? <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.siteRail}>
        {sites.map(site => <Pressable key={site.id} onPress={() => setSelectedSite(site.id)} style={[styles.siteChip, site.id === selectedSite && styles.siteChipSelected]}><Text style={[styles.siteChipText, site.id === selectedSite && styles.siteChipTextSelected]}>{site.name}{site.local ? ' · lokal' : site.online ? ' · online' : ' · offline'}</Text></Pressable>)}
      </ScrollView> : null}
      {error ? <View style={styles.errorBox}><Text style={styles.error}>{error}</Text><Pressable onPress={() => void refresh()}><Text style={styles.retry}>Erneut versuchen</Text></Pressable></View> : null}
      <View style={styles.sectionHeading}><Text style={styles.sectionTitle}>Alle Geräte</Text><Text style={styles.count}>{devices.length}</Text></View>
      {!refreshing && devices.length === 0 && !error ? <View style={styles.empty}><View style={styles.emptyIcon}><Text style={styles.computerGlyph}>▣</Text></View><Text style={styles.emptyTitle}>{sites.length ? 'Noch keine Geräte' : 'Kein Standort verbunden'}</Text><Text style={styles.emptyText}>{sites.length ? 'Füge Geräte über die Web-Oberfläche deiner WOL UI Instanz hinzu.' : 'Melde dich an deiner WOL UI Instanz an, um verbundene Standorte zu sehen.'}</Text></View> : null}
      {sortedDevices.map(device => {
        const job = jobs[device.id]
        const isSending = waking === device.id || ['queued', 'sending', 'pending', 'accepted'].includes(job?.status ?? '')
        return <View key={device.id} style={styles.deviceCard}>
          <View style={styles.deviceTop}>
            <View style={styles.deviceIcon}><Text style={styles.computerGlyph}>▣</Text></View>
            <View style={styles.deviceInfo}><Text numberOfLines={1} style={styles.deviceName}>{device.name}</Text><Text style={styles.deviceMac}>{device.mac}</Text></View>
            <View style={styles.profileTag}><Text numberOfLines={1} style={styles.profileText}>{profiles.find(profile => profile.id === device.network_profile_id)?.name ?? 'LAN'}</Text></View>
          </View>
          {device.tailscale_name ? <Pressable accessibilityRole="link" accessibilityLabel={`Browser-SSH-Konsole für ${device.name} öffnen`} onPress={() => void Linking.openURL('https://console.tailscale.com/admin/machines')}><Text style={styles.tailscale}>⌁  {device.tailscale_name} · Browser-SSH in Tailscale öffnen ↗</Text></Pressable> : null}
          {job && !isSending ? <Text style={[styles.jobStatus, job.status === 'failed' ? styles.jobFailed : job.status === 'sent' ? styles.jobSent : null]}>{statusLabel(job.status)}</Text> : null}
          <Pressable accessibilityRole="button" accessibilityLabel={device.name + ' starten'} onPress={() => void wakeDevice(device)} disabled={isSending || !selectedSiteOnline} style={({ pressed }) => [styles.wakeButton, pressed && styles.buttonPressed, (isSending || !selectedSiteOnline) && styles.disabledButton]}>
            {isSending ? <ActivityIndicator size="small" color="#23744c" /> : <Text style={styles.powerGlyphSmall}>⏻</Text>}
            <Text style={styles.wakeText}>{isSending ? 'Startsignal wird gesendet' : selectedSiteOnline ? 'Gerät starten' : 'Standort offline'}</Text>
          </Pressable>
        </View>
      })}
      <Text style={styles.footer}>WOL UI  ·  Lokal und sicher</Text>
    </ScrollView>
  </SafeAreaView>
}

function Brand() {
  return <View style={styles.brand}><View style={styles.brandMark}><Text style={styles.brandPower}>⏻</Text></View><Text style={styles.brandName}>WOL<Text style={styles.brandLight}>UI</Text></Text></View>
}

function Centered({ children }: { children: React.ReactNode }) {
  return <SafeAreaView style={styles.safe}><View style={styles.centered}>{children}</View></SafeAreaView>
}

function statusLabel(status: string) {
  if (status === 'sent') return 'Startsignal gesendet'
  if (status === 'failed') return 'Senden fehlgeschlagen'
  if (status === 'unknown') return 'Ergebnis unklar'
  return status
}

const styles = StyleSheet.create({
  safe: { flex: 1, backgroundColor: '#f4f6f2' },
  centered: { flex: 1, alignItems: 'center', justifyContent: 'center', gap: 12 },
  bootText: { color: '#77837b', fontSize: 13 },
  loginScroll: { flexGrow: 1, justifyContent: 'center', padding: 22 },
  loginCard: { backgroundColor: '#fff', borderColor: '#e5eae4', borderWidth: 1, borderRadius: 22, padding: 24, shadowColor: '#183726', shadowOpacity: 0.06, shadowRadius: 24, shadowOffset: { width: 0, height: 12 }, elevation: 3 },
  brand: { flexDirection: 'row', alignItems: 'center', gap: 10 },
  brandMark: { width: 36, height: 36, alignItems: 'center', justifyContent: 'center', borderRadius: 12, backgroundColor: '#23744c' },
  brandPower: { color: '#fff', fontSize: 20, fontWeight: '800' },
  brandName: { color: '#193b2b', fontSize: 19, fontWeight: '800', letterSpacing: -0.8 },
  brandLight: { color: '#81968a', fontWeight: '500' },
  eyebrow: { color: '#768b7b', fontSize: 10, fontWeight: '800', letterSpacing: 1.5 },
  loginTitle: { marginTop: 10, color: '#17211d', fontSize: 30, fontWeight: '800', letterSpacing: -1.1, lineHeight: 35 },
  description: { color: '#77827b', fontSize: 13, lineHeight: 20, marginTop: 11, marginBottom: 25 },
  label: { color: '#425147', fontSize: 12, fontWeight: '700', marginBottom: 7, marginTop: 13 },
  input: { height: 48, backgroundColor: '#fbfcfa', borderWidth: 1, borderColor: '#e1e7e0', borderRadius: 10, paddingHorizontal: 13, color: '#26372d', fontSize: 13 },
  primaryButton: { height: 48, backgroundColor: '#23744c', borderRadius: 10, marginTop: 19, alignItems: 'center', justifyContent: 'center' },
  primaryButtonText: { color: '#fff', fontWeight: '800', fontSize: 13 },
  buttonPressed: { opacity: 0.82 },
  disabledButton: { opacity: 0.52 },
  error: { color: '#aa4e45', fontSize: 12, lineHeight: 18 },
  loginFoot: { textAlign: 'center', color: '#8a958d', fontSize: 10, marginTop: 17 },
  header: { height: 61, paddingHorizontal: 20, flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', borderBottomWidth: 1, borderBottomColor: '#e8ece6', backgroundColor: '#fff' },
  signOut: { paddingVertical: 9, paddingHorizontal: 12, borderRadius: 8, backgroundColor: '#f4f6f3' },
  signOutText: { color: '#66756b', fontSize: 11, fontWeight: '700' },
  content: { padding: 20, paddingBottom: 32 },
  headingRow: { flexDirection: 'row', alignItems: 'flex-end', justifyContent: 'space-between', marginBottom: 20 },
  title: { color: '#17211d', fontSize: 30, fontWeight: '800', letterSpacing: -1.1, marginTop: 5 },
  refreshButton: { paddingVertical: 9, paddingHorizontal: 11, backgroundColor: '#fff', borderColor: '#e1e7e0', borderWidth: 1, borderRadius: 8 },
  refreshText: { color: '#53645a', fontSize: 11, fontWeight: '700' },
  summary: { minHeight: 84, padding: 16, backgroundColor: '#fff', borderColor: '#e7ece6', borderWidth: 1, borderRadius: 14, flexDirection: 'row', alignItems: 'center', gap: 13 },
  summaryIcon: { width: 42, height: 42, borderRadius: 12, alignItems: 'center', justifyContent: 'center', backgroundColor: '#edf5ef' },
  powerGlyph: { color: '#2a7650', fontSize: 23, fontWeight: '600' },
  summaryCopy: { flex: 1 },
  summaryTitle: { color: '#23342a', fontSize: 13, fontWeight: '800' },
  summaryHint: { color: '#879289', fontSize: 11, marginTop: 4 },
  liveDot: { width: 8, height: 8, borderRadius: 4, backgroundColor: '#61a674' },
  offlineDot: { backgroundColor: '#c49b53' },
  siteRail: { gap: 8, paddingVertical: 16 },
  siteChip: { borderRadius: 20, backgroundColor: '#fff', borderColor: '#e3e8e2', borderWidth: 1, paddingVertical: 8, paddingHorizontal: 13 },
  siteChipSelected: { backgroundColor: '#e9f3ec', borderColor: '#b9d7c1' },
  siteChipText: { color: '#718078', fontSize: 11, fontWeight: '700' },
  siteChipTextSelected: { color: '#26704a' },
  errorBox: { backgroundColor: '#fff6f4', borderColor: '#f0d8d4', borderWidth: 1, borderRadius: 10, padding: 12, marginBottom: 16, gap: 8 },
  retry: { color: '#23744c', fontSize: 11, fontWeight: '800' },
  sectionHeading: { flexDirection: 'row', alignItems: 'center', gap: 8, marginTop: 26, marginBottom: 12 },
  sectionTitle: { color: '#26372d', fontSize: 15, fontWeight: '800' },
  count: { overflow: 'hidden', color: '#718078', backgroundColor: '#e8eee8', borderRadius: 12, paddingHorizontal: 7, paddingVertical: 3, fontSize: 10, fontWeight: '700' },
  empty: { minHeight: 210, alignItems: 'center', justifyContent: 'center', paddingHorizontal: 25, backgroundColor: '#fbfcfa', borderColor: '#dfe6de', borderWidth: 1, borderStyle: 'dashed', borderRadius: 14 },
  emptyIcon: { width: 43, height: 43, borderRadius: 13, backgroundColor: '#edf4ee', alignItems: 'center', justifyContent: 'center', marginBottom: 12 },
  computerGlyph: { color: '#688270', fontSize: 21, fontWeight: '700' },
  emptyTitle: { color: '#304136', fontSize: 13, fontWeight: '800' },
  emptyText: { color: '#87928a', fontSize: 11, lineHeight: 17, marginTop: 7, textAlign: 'center' },
  deviceCard: { backgroundColor: '#fff', borderColor: '#e7ece6', borderWidth: 1, borderRadius: 13, padding: 14, marginBottom: 10 },
  deviceTop: { flexDirection: 'row', alignItems: 'center', gap: 10 },
  deviceIcon: { width: 38, height: 38, borderRadius: 11, backgroundColor: '#f0f4f0', alignItems: 'center', justifyContent: 'center' },
  deviceInfo: { flex: 1 },
  deviceName: { color: '#25372c', fontSize: 13, fontWeight: '800' },
  deviceMac: { color: '#89948c', fontSize: 10, marginTop: 5 },
  profileTag: { maxWidth: 90, paddingVertical: 5, paddingHorizontal: 8, backgroundColor: '#f4f6f3', borderRadius: 12 },
  profileText: { color: '#758179', fontSize: 9, fontWeight: '700' },
  tailscale: { color: '#64806d', fontSize: 10, marginTop: 11 },
  jobStatus: { color: '#6f7e74', fontSize: 10, fontWeight: '700', marginTop: 10 },
  jobFailed: { color: '#a85147' },
  jobSent: { color: '#3d8053' },
  wakeButton: { minHeight: 42, backgroundColor: '#eff6f0', borderColor: '#d8e8dc', borderWidth: 1, borderRadius: 9, marginTop: 13, flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 7 },
  powerGlyphSmall: { color: '#26744c', fontSize: 17, fontWeight: '700' },
  wakeText: { color: '#2e704b', fontSize: 11, fontWeight: '800' },
  footer: { color: '#9aa39d', fontSize: 10, textAlign: 'center', marginTop: 18 },
})

export default App
