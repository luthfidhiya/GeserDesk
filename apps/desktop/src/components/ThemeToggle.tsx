import { Moon, Sun } from 'lucide-react'
import { Button } from '@/components/ui/button'
import type { Theme } from '@/types'

/** What the page actually looks like now; `system` resolves to the OS setting. */
const isDark = (theme: Theme) =>
  theme === 'dark' || (theme === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)

/** A sun in the dark theme, a moon in the light one; a click switches to the other. */
export function ThemeToggle({ theme, onChange }: { theme: Theme; onChange: (t: Theme) => void }) {
  const dark = isDark(theme)
  const label = dark ? 'Ganti ke tema terang' : 'Ganti ke tema gelap'
  return (
    <Button variant="ghost" size="icon" aria-label={label} title={label} onClick={() => onChange(dark ? 'light' : 'dark')}>
      {dark ? <Sun className="size-4" /> : <Moon className="size-4" />}
    </Button>
  )
}
