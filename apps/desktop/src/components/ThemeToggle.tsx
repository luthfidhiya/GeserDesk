import { Moon, Sun } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useT } from '@/lib/i18n'
import type { Theme } from '@/types'

/** What the page actually looks like now; `system` resolves to the OS setting. */
const isDark = (theme: Theme) =>
  theme === 'dark' || (theme === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)

/** A sun in the dark theme, a moon in the light one; a click switches to the other. */
export function ThemeToggle({ theme, onChange }: { theme: Theme; onChange: (t: Theme) => void }) {
  const dark = isDark(theme)
  const t = useT()
  const label = dark ? t('Ganti ke tema terang', 'Switch to the light theme') : t('Ganti ke tema gelap', 'Switch to the dark theme')
  return (
    <Button variant="ghost" size="icon" aria-label={label} title={label} onClick={() => onChange(dark ? 'light' : 'dark')}>
      {dark ? <Sun className="size-4" /> : <Moon className="size-4" />}
    </Button>
  )
}
