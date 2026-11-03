import type { PropsWithChildren } from 'react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { ThemePreference } from '../domain/appearance';
import { I18nProvider, resolveLocale } from '../i18n';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { ThemePicker } from '../ui/ThemePicker';
import styles from './DocsLayout.module.css';

/** Documentation consumes product tokens; Canvas stories keep their own iframe context. */
export function DocsLayout({ children }: PropsWithChildren) {
  const [theme, setTheme] = useState<ThemePreference>('light');
  const { i18n } = useTranslation();
  const article = useRef<HTMLElement>(null);
  useEffect(() => {
    const root = article.current;
    if (!root) return;
    const doc = root.ownerDocument;
    const nativeHeadingLink = (event: MouseEvent) => {
      const target = event.target as Element | null;
      const link = target?.closest?.<HTMLAnchorElement>('.toc-wrapper a[href^="#"]');
      if (!link) return;
      const heading = doc.getElementById(decodeURIComponent(link.hash.slice(1)));
      // Resolve the decoded heading before Tocbot's lookup and preserve native hash links.
      if (heading && root.contains(heading)) {
        event.preventDefault();
        event.stopImmediatePropagation();
        doc.defaultView?.history.replaceState(null, '', link.hash);
        heading.scrollIntoView({ block: 'start' });
        heading.tabIndex = -1;
        heading.focus({ preventScroll: true });
      }
    };
    doc.addEventListener('click', nativeHeadingLink, true);
    return () => doc.removeEventListener('click', nativeHeadingLink, true);
  }, []);
  return (
    <MixRelayProvider theme={theme} onThemeChange={setTheme}>
      <I18nProvider>
        <Unstyled>
          <article ref={article} lang="ko-KR" className={styles.root}>
            <div className={styles.toolbar}>
              <span>MixRelay · 디자인 시스템 · 문서 테마 {theme}</span>
              <div lang={resolveLocale(i18n.resolvedLanguage ?? i18n.language)}>
                <ThemePicker />
              </div>
            </div>
            {children}
          </article>
        </Unstyled>
      </I18nProvider>
    </MixRelayProvider>
  );
}

import { Unstyled } from '@storybook/addon-docs/blocks';
