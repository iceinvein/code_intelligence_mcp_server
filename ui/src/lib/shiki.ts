import type { Highlighter } from "shiki";

const LANGS = ["typescript", "tsx", "rust", "python", "go", "java", "c", "cpp"];
const LIGHT = "github-light-default";
const DARK = "github-dark-default";

let highlighterPromise: Promise<Highlighter> | null = null;

function loadHighlighter(): Promise<Highlighter> {
  if (!highlighterPromise) {
    highlighterPromise = import("shiki")
      .then((s) => s.createHighlighter({ themes: [LIGHT, DARK], langs: LANGS }))
      .catch((e) => {
        // Allow a retry on the next call rather than caching the failure for the page lifetime.
        highlighterPromise = null;
        return Promise.reject(e);
      });
  }
  return highlighterPromise;
}

/// Highlight `code` to dual-theme HTML. Falls back to "text" for unknown langs.
export async function highlight(code: string, lang: string): Promise<string> {
  const hl = await loadHighlighter();
  const safeLang = hl.getLoadedLanguages().includes(lang) ? lang : "text";
  return hl.codeToHtml(code, {
    lang: safeLang,
    themes: { light: LIGHT, dark: DARK },
    defaultColor: false,
  });
}
