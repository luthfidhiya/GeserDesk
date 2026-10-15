import { open } from '@tauri-apps/plugin-dialog'
import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { TuningForm } from '@/components/TuningForm'
import { errorText, ipc } from '@/lib/ipc'
import type { Settings } from '@/types'
import { useT } from '@/lib/i18n'

interface Props {
  settings: Settings
  disabled: boolean
  onChange: (change: (s: Settings) => Settings) => void
  onProblem: (text: string | null) => void
}

export function ClientForm({ settings, disabled, onChange }: Props) {
  const t = useT()
  const c = settings.client
  const set = (patch: Partial<Settings['client']>) =>
    onChange((s) => ({ ...s, client: { ...s.client, ...patch } }))
  return (
    <div className="grid grid-cols-[10rem_1fr] items-center gap-x-3 gap-y-3">
      <Label htmlFor="srv">{t('Alamat server', 'Server address')}</Label>
      <Input
        id="srv"
        disabled={disabled}
        value={c.server}
        placeholder={t('contoh 192.168.100.10 atau 192.168.100.10:24810', 'e.g. 192.168.100.10 or 192.168.100.10:24810')}
        onChange={(e) => set({ server: e.target.value })}
      />
      <Label htmlFor="nm">{t('Nama layar ini', 'This screen\'s name')}</Label>
      <Input id="nm" disabled={disabled} value={c.name} onChange={(e) => set({ name: e.target.value })} />
      <span />
      <div className="flex items-center gap-2">
        <Checkbox
          id="allow-control"
          disabled={disabled}
          checked={!c.log_only}
          onCheckedChange={(v) => set({ log_only: v !== true })}
        />
        <Label htmlFor="allow-control" className="flex-wrap">
          {t('Izinkan dikendalikan server', 'Let the server control this computer')}
          <span className="font-normal text-muted-foreground">
            {t('(tanpa centang, input hanya dicatat di log)', '(unticked, input is only written to the log)')}
          </span>
        </Label>
      </div>
    </div>
  )
}

export function ServerForm({ settings, disabled, onChange, onProblem, bare }: Props & { bare?: boolean }) {
  const t = useT()
  const path = settings.server.config
  const [exists, setExists] = useState(true)
  const [defaultPath, setDefaultPath] = useState<string | null>(null)

  useEffect(() => {
    void ipc.defaultConfigPath().then(setDefaultPath)
  }, [])

  useEffect(() => {
    let current = true
    void ipc.configExists(path.trim()).then((e) => current && setExists(e))
    return () => {
      current = false
    }
  }, [path])

  const set = (config: string) => onChange((s) => ({ ...s, server: { ...s.server, config } }))

  const browse = async () => {
    const picked = await open({ multiple: false, filters: [{ name: 'TOML', extensions: ['toml'] }] })
    if (typeof picked === 'string') set(picked)
  }

  const create = async () => {
    try {
      await ipc.createDefaultConfig(path.trim())
      onProblem(null)
      setExists(true)
    } catch (e) {
      onProblem(errorText(e))
    }
  }

  // The default file is made by Start, so it needs no setting. Choosing another
  // file is the exception, so that part stays shut unless it is in use or missing.
  // The choice is its own state, so "Berkas lain" can be picked while the path
  // still equals the default.
  const [choice, setChoice] = useState<'default' | 'custom' | null>(null)
  useEffect(() => {
    if (defaultPath !== null && choice === null) setChoice(path.trim() === defaultPath ? 'default' : 'custom')
  }, [defaultPath, choice, path])

  const custom = choice === 'custom'
  const needsAttention = custom && !exists

  const pick = (next: 'default' | 'custom') => {
    setChoice(next)
    if (next === 'default' && defaultPath) set(defaultPath)
  }

  // In the main window: the two choices and a button to the advanced settings
  // window. In that window (`bare`) the port and switching settings follow.
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
        <span className="text-sm">{t('Berkas konfigurasi', 'Configuration file')}</span>
        <RadioGroup
          className="flex items-center gap-6"
          value={choice ?? 'default'}
          onValueChange={(v) => pick(v as 'default' | 'custom')}
          disabled={disabled}
        >
          <div className="flex items-center gap-2">
            <RadioGroupItem value="default" id="cfg-default" />
            <Label htmlFor="cfg-default">{t('Bawaan', 'Default')}</Label>
          </div>
          <div className="flex items-center gap-2">
            <RadioGroupItem value="custom" id="cfg-custom" />
            <Label htmlFor="cfg-custom">{t('Berkas lain', 'Another file')}</Label>
          </div>
        </RadioGroup>
        {!bare && (
          <Button variant="outline" size="sm" className="ml-auto" onClick={() => void ipc.openTool('settings')}>
            {t('Pengaturan lanjutan', 'Advanced settings')}
          </Button>
        )}
      </div>
      {custom && (
        <div className="grid grid-cols-[1fr_auto] items-center gap-x-3">
          <Input
            id="cfg"
            aria-label={t('Path berkas konfigurasi', 'Configuration file path')}
            disabled={disabled}
            value={path}
            onChange={(e) => set(e.target.value)}
          />
          <Button variant="outline" disabled={disabled} onClick={() => void browse()}>
            {t('Pilih…', 'Browse…')}
          </Button>
        </div>
      )}
      {needsAttention && !disabled && (
        <Button variant="secondary" onClick={() => void create()}>
          {t('Buat berkas konfigurasi awal', 'Create a starting configuration file')}
        </Button>
      )}
      {bare && !needsAttention && <TuningForm path={path} disabled={disabled} />}
    </div>
  )
}
