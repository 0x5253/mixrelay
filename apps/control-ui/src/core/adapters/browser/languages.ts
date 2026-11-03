/** The browser's language preferences in order, then its primary language. */
export function browserLanguages(page: Pick<Navigator, 'languages' | 'language'>): string[] {
  return [...page.languages, page.language];
}
