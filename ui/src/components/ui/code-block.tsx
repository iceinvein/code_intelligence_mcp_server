import { useEffect, useState } from "react";
import { highlight } from "@/lib/shiki";

type CodeBlockProps = { code: string; lang: string };

export function CodeBlock({ code, lang }: CodeBlockProps) {
  const [html, setHtml] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setHtml(null);
    highlight(code, lang)
      .then((h) => {
        if (alive) setHtml(h);
      })
      .catch(() => {
        /* fall back to the plain <pre> below */
      });
    return () => {
      alive = false;
    };
  }, [code, lang]);

  if (html) {
    // The injected .shiki <pre> already scrolls horizontally (see index.css), so no overflow wrapper here.
    return <div dangerouslySetInnerHTML={{ __html: html }} />;
  }
  return (
    <pre className="overflow-x-auto whitespace-pre rounded p-2 font-mono text-[11px] text-muted-foreground">
      {code}
    </pre>
  );
}
