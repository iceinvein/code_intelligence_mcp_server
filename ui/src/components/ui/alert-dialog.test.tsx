import { test, expect, afterEach, mock } from "bun:test";
import { render, fireEvent, act } from "@testing-library/react";
import {
  AlertDialog,
  AlertDialogTrigger,
  AlertDialogContent,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogAction,
  AlertDialogCancel,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";

afterEach(() => mock.restore());

test("AlertDialog opens from trigger and fires the action handler", async () => {
  let acted = false;
  const { findByText, getByText } = render(
    <AlertDialog>
      <AlertDialogTrigger render={<Button>drop</Button>} />
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Drop?</AlertDialogTitle>
          <AlertDialogDescription>cannot be undone</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>cancel</AlertDialogCancel>
          <AlertDialogAction onClick={() => (acted = true)}>confirm</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>,
  );

  await act(async () => {
    fireEvent.click(getByText("drop"));
  });
  const confirm = await findByText("confirm");
  await act(async () => {
    fireEvent.click(confirm);
  });
  expect(acted).toBe(true);
});
