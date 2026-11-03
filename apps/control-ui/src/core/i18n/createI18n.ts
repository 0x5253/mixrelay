import i18next, { type i18n as I18n, type ThirdPartyModule } from 'i18next';
import { resolveLocale } from './locale';
import { de } from './locales/de';
import { en } from './locales/en';
import { es } from './locales/es';
import { fr } from './locales/fr';
import { it } from './locales/it';
import { ja } from './locales/ja';
import { koKR } from './locales/ko-KR';
import { pseudo } from './locales/pseudo';
import { pt } from './locales/pt';
import { ru } from './locales/ru';
import { zhHans } from './locales/zh-Hans';
import { zhHant } from './locales/zh-Hant';

const resources = {
  en: { translation: en },
  'ko-KR': { translation: koKR },
  ja: { translation: ja },
  'zh-Hans': { translation: zhHans },
  'zh-Hant': { translation: zhHant },
  fr: { translation: fr },
  de: { translation: de },
  es: { translation: es },
  pt: { translation: pt },
  it: { translation: it },
  ru: { translation: ru },
  pseudo: { translation: pseudo },
};

/** An independent translation instance in the best supported language for `languages`,
 * which are the user's preferences in order. Plugins initialise with it. */
export function createI18n(languages: readonly string[], ...plugins: ThirdPartyModule[]): I18n {
  const instance = i18next.createInstance();
  for (const plugin of plugins) instance.use(plugin);
  void instance.init({
    fallbackLng: 'en',
    interpolation: { escapeValue: false },
    lng: resolveLocale(languages),
    resources,
  });
  return instance;
}
