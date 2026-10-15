import { useT } from '@/lib/i18n'

/**
 * The status dot of a client's screen, which is also its switch: green while connected,
 * grey while not, a red ring when the screen is off. Clicking turns it off or on.
 */
export function ScreenSwitch({
  name,
  connected,
  disabled,
  onToggle,
}: {
  name: string
  connected: boolean
  disabled: boolean
  onToggle: (disabled: boolean) => void
}) {
  const t = useT()
  return (
    <button
      className="grid size-6 place-items-center rounded hover:bg-muted"
      title={
        disabled
          ? t('Nonaktif. Klik untuk mengaktifkan', 'Off. Click to turn on')
          : connected
            ? t('Terhubung. Klik untuk menonaktifkan', 'Connected. Click to turn off')
            : t('Tidak terhubung. Klik untuk menonaktifkan', 'Not connected. Click to turn off')
      }
      aria-label={disabled ? t(`Aktifkan ${name}`, `Turn on ${name}`) : t(`Nonaktifkan ${name}`, `Turn off ${name}`)}
      onClick={(e) => {
        e.stopPropagation()
        onToggle(!disabled)
      }}
    >
      <span
        className="size-2.5 rounded-full"
        style={disabled ? { border: '2px solid var(--bad)' } : { background: connected ? 'var(--ok)' : 'var(--muted-foreground)' }}
      />
    </button>
  )
}
