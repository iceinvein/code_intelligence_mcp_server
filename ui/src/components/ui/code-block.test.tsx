import { test, expect, mock, afterEach } from "bun:test";
import { render, waitFor } from "@testing-library/react";

mock.module("@/lib/shiki", () => ({
  highlight: async (code: string) => `<pre class="shiki"><code>HL:${code}</code></pre>`,
}));

afterEach(() => {
  mock.restore();
});

test("CodeBlock shows plain code, then the highlighted markup", async () => {
  const { CodeBlock } = await import("@/components/ui/code-block");
  const { container } = render(<CodeBlock code="fn foo() {}" lang="rust" />);

  // Plain fallback renders the raw code immediately.
  expect(container.textContent).toContain("fn foo() {}");
  // After the mocked highlight resolves, the shiki markup appears.
  await waitFor(() => expect(container.querySelector(".shiki")).not.toBeNull());
  expect(container.textContent).toContain("HL:fn foo() {}");
});
