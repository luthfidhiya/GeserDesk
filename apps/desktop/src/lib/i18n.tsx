import { createContext, useContext } from 'react'

/** The languages the window speaks (kept in step with `Language` in Rust). */
export type Lang = 'id' | 'en'

const LangContext = createContext<Lang>('id')

/** Every page is wrapped in one, with the language from the saved settings. */
export const LangProvider = LangContext.Provider

export const useLang = () => useContext(LangContext)

/**
 * The words in the window's language. Each text is written in both languages where it is
 * used, `t('Simpan', 'Save')`, so a text and its translation never drift apart.
 */
export function useT() {
  const lang = useLang()
  return (id: string, en: string) => (lang === 'en' ? en : id)
}

/** The language now, for code that is neither a component nor handed it (error messages). */
let current: Lang = 'id'
export const currentLang = () => current
export const setCurrentLang = (lang: Lang) => {
  current = lang
}

/** The same, for code that is not a component and is handed the language. */
export const pick = (lang: Lang, id: string, en: string) => (lang === 'en' ? en : id)
