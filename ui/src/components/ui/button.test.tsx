import { test, expect, afterEach, mock } from "bun:test";
import { render } from "@testing-library/react";
import { Button } from "@/components/ui/button";

afterEach(() => mock.restore());

test("Button renders a native button by default", () => {
  const { getByRole } = render(<Button>click me</Button>);
  const el = getByRole("button");
  expect(el.tagName).toBe("BUTTON");
  expect(el.textContent).toBe("click me");
});

test("Button render prop composes a different element (anchor)", () => {
  const { getByText } = render(
    <Button render={<a href="/x" />} variant="outline">
      go
    </Button>,
  );
  const link = getByText("go");
  expect(link.tagName).toBe("A");
  expect(link.getAttribute("href")).toBe("/x");
});
