// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { SessionViewConversionDialog } from "../SessionViewConversionDialog";

afterEach(() => {
  cleanup();
});

describe("SessionViewConversionDialog", () => {
  it("does not convert when Cancel is clicked", () => {
    const onConfirm = vi.fn().mockResolvedValue(true);
    const onCancel = vi.fn();

    render(<SessionViewConversionDialog sessionTitle="codex" onConfirm={onConfirm} onCancel={onCancel} />);

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(onConfirm).not.toHaveBeenCalled();
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("warns about transcript deletion and confirms once", async () => {
    const onConfirm = vi.fn().mockResolvedValue(true);
    const onCancel = vi.fn();

    render(<SessionViewConversionDialog sessionTitle="codex" onConfirm={onConfirm} onCancel={onCancel} />);

    expect(screen.getByText(/structured transcript will be deleted/i)).not.toBeNull();
    expect(screen.getByText(/previous terminal is not restored/i)).not.toBeNull();
    expect(screen.getByText(/new terminal starts fresh/i)).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Convert to terminal" }));

    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledTimes(1));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("cancels when Escape is pressed", () => {
    const onCancel = vi.fn();

    render(
      <SessionViewConversionDialog
        sessionTitle="codex"
        onConfirm={vi.fn().mockResolvedValue(true)}
        onCancel={onCancel}
      />,
    );

    fireEvent.keyDown(document, { key: "Escape" });

    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("confirms when Enter is pressed away from a button", async () => {
    const onConfirm = vi.fn().mockResolvedValue(false);

    render(<SessionViewConversionDialog sessionTitle="codex" onConfirm={onConfirm} onCancel={vi.fn()} />);

    fireEvent.keyDown(document, { key: "Enter" });

    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledTimes(1));
  });

  it("cancels when the backdrop is clicked", () => {
    const onCancel = vi.fn();

    render(
      <SessionViewConversionDialog
        sessionTitle="codex"
        onConfirm={vi.fn().mockResolvedValue(true)}
        onCancel={onCancel}
      />,
    );

    fireEvent.click(screen.getByTestId("session-view-conversion-dialog"));

    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("keeps the dialog open and re-enables controls after a failed confirmation", async () => {
    const onConfirm = vi.fn().mockResolvedValue(false);
    const onCancel = vi.fn();

    render(<SessionViewConversionDialog sessionTitle="codex" onConfirm={onConfirm} onCancel={onCancel} />);

    fireEvent.click(screen.getByRole("button", { name: "Convert to terminal" }));

    await vi.waitFor(() => {
      expect((screen.getByRole("button", { name: "Convert to terminal" }) as HTMLButtonElement).disabled).toBe(false);
    });
    expect(screen.getByTestId("session-view-conversion-dialog")).not.toBeNull();
    expect((screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(false);
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("keeps the dialog open and re-enables controls after a rejected confirmation", async () => {
    const onConfirm = vi.fn().mockRejectedValue(new Error("network failure"));
    const onCancel = vi.fn();

    render(<SessionViewConversionDialog sessionTitle="codex" onConfirm={onConfirm} onCancel={onCancel} />);

    fireEvent.click(screen.getByRole("button", { name: "Convert to terminal" }));

    await vi.waitFor(() => {
      expect((screen.getByRole("button", { name: "Convert to terminal" }) as HTMLButtonElement).disabled).toBe(false);
    });
    expect(screen.getByTestId("session-view-conversion-dialog")).not.toBeNull();
    expect((screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(false);
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("prevents cancellation and a second conversion while confirmation is in flight", () => {
    let resolveConfirm: ((result: boolean) => void) | undefined;
    const onConfirm = vi.fn(
      () =>
        new Promise<boolean>((resolve) => {
          resolveConfirm = resolve;
        }),
    );
    const onCancel = vi.fn();

    render(<SessionViewConversionDialog sessionTitle="codex" onConfirm={onConfirm} onCancel={onCancel} />);

    fireEvent.click(screen.getByRole("button", { name: "Convert to terminal" }));
    fireEvent.keyDown(document, { key: "Enter" });
    fireEvent.keyDown(document, { key: "Escape" });
    fireEvent.click(screen.getByTestId("session-view-conversion-dialog"));

    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onCancel).not.toHaveBeenCalled();
    expect((screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(true);
    resolveConfirm?.(false);
  });

  it("focuses conversion on mount and restores the trigger on unmount", () => {
    const trigger = document.createElement("button");
    document.body.appendChild(trigger);
    trigger.focus();

    const { unmount } = render(
      <SessionViewConversionDialog
        sessionTitle="codex"
        onConfirm={vi.fn().mockResolvedValue(true)}
        onCancel={vi.fn()}
      />,
    );

    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Convert to terminal" }));
    unmount();
    expect(document.activeElement).toBe(trigger);
    trigger.remove();
  });
});
