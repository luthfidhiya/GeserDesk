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
  return (
    <button
      className="grid size-6 place-items-center rounded hover:bg-muted"
      title={
        disabled ? 'Nonaktif. Klik untuk mengaktifkan' : `${connected ? 'Terhubung' : 'Tidak terhubung'}. Klik untuk menonaktifkan`
      }
      aria-label={disabled ? `Aktifkan ${name}` : `Nonaktifkan ${name}`}
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
