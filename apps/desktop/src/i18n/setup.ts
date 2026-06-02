import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import en from './en.json';
import ru from './ru.json';

export type Locale = 'ru' | 'en';
const STORAGE_KEY = 'nexus.locale';

/**
 * Определяет локаль при первом запуске: сохранённый выбор → системная локаль
 * (`navigator.language`) → 'en'. Так новый русскоязычный пользователь сразу видит ru (С-11).
 */
export function detectLocale(): Locale {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === 'ru' || stored === 'en') return stored;
  } catch {
    /* localStorage недоступен — игнорируем */
  }
  const sys = (typeof navigator !== 'undefined' && navigator.language) || 'en';
  return sys.toLowerCase().startsWith('ru') ? 'ru' : 'en';
}

if (!i18n.isInitialized) {
  void i18n.use(initReactI18next).init({
    resources: { ru: { translation: ru }, en: { translation: en } },
    lng: detectLocale(),
    fallbackLng: 'en',
    interpolation: { escapeValue: false }, // React сам экранирует
  });
}

/** Переключает язык (без потери состояния сторов) и запоминает выбор. */
export function changeLocale(locale: Locale): void {
  try {
    localStorage.setItem(STORAGE_KEY, locale);
  } catch {
    /* ignore */
  }
  void i18n.changeLanguage(locale);
}

export default i18n;
