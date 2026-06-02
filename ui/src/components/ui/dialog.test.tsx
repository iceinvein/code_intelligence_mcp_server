import { test, expect, afterEach, mock } from "bun:test";
import { render } from "@testing-library/react";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";

afterEach(() => mock.restore());

test("controlled Dialog renders its content when open", async () => {
  const { findByText } = render(
    <Dialog open onOpenChange={() => {}}>
      <DialogContent>
        <DialogTitle>Pick a folder</DialogTitle>
      </DialogContent>
    </Dialog>,
  );
  expect(await findByText("Pick a folder")).toBeDefined();
});

test("closed Dialog does not render content", () => {
  const { queryByText } = render(
    <Dialog open={false} onOpenChange={() => {}}>
      <DialogContent>
        <DialogTitle>Hidden</DialogTitle>
      </DialogContent>
    </Dialog>,
  );
  expect(queryByText("Hidden")).toBeNull();
});
