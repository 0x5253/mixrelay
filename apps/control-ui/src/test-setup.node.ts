// DOM-free tests run in Node. Only the app i18n instance is shared with runtime code;
// DOM matchers, cleanup and browser stubs live in test-setup.ts for the jsdom project.
import { i18n } from './core/i18n';

// Node reports the host locale through navigator.languages, while jsdom reports en-US.
// Pin the same language so results do not depend on the machine running them.
await i18n.changeLanguage('en');
