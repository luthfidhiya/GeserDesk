import { useEffect } from 'react'
import type { Theme } from '@/types'

/** Apply the chosen theme to the page; `system` follows the operating system. */
export function useTheme(theme: Theme | undefined) {
  useEffect(() => {
    if (!theme) return
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const apply = () =>
      document.documentElement.classList.toggle('dark', theme === 'dark' || (theme === 'system' && media.matches))
    apply()
    if (theme !== 'system') return
    media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [theme])
}
