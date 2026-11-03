import korean from '../ko.json' with { type: 'json' };

export function translate(language, key) {
  return (language?.startsWith('ko') ? korean.Localization[key] : undefined) ?? key;
}
