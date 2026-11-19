import { createContext, useContext } from "react";
import type { AppLanguage } from "../types";
import { en } from "./en";
import { ru, type Translations } from "./ru";

export type { Translations };

const DICTS: Record<AppLanguage, Translations> = {
  ru,
  en,
};

export function getTranslations(lang: AppLanguage): Translations {
  return lang === "en" ? DICTS.en : DICTS.ru;
}

interface I18nContextValue {
  lang: AppLanguage;
  t: Translations;
}

const I18nContext = createContext<I18nContextValue>({
  lang: "ru",
  t: ru,
});

export function I18nProvider({
  lang,
  children,
}: {
  lang: AppLanguage;
  children: React.ReactNode;
}) {
  return (
    <I18nContext.Provider value={{ lang, t: getTranslations(lang) }}>
      {children}
    </I18nContext.Provider>
  );
}

export function useI18n(): I18nContextValue {
  return useContext(I18nContext);
}
