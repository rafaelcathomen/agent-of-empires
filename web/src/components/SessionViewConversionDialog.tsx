import { useCallback, useEffect, useRef, useState } from "react";

interface Props {
  sessionTitle: string;
  onConfirm: () => Promise<boolean>;
  onCancel: () => void;
}

export function SessionViewConversionDialog({ sessionTitle, onConfirm, onCancel }: Props) {
  const [converting, setConverting] = useState(false);
  const confirmButtonRef = useRef<HTMLButtonElement | null>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);

  const handleConfirm = useCallback(async () => {
    if (converting) return;

    setConverting(true);
    try {
      if (await onConfirm()) {
        onCancel();
        return;
      }
    } catch {
      // Keep the dialog open so the caller's error state remains actionable.
    }
    setConverting(false);
  }, [converting, onCancel, onConfirm]);

  useEffect(() => {
    previousFocusRef.current = document.activeElement as HTMLElement | null;
    confirmButtonRef.current?.focus();
    return () => {
      previousFocusRef.current?.focus?.();
    };
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (!converting) onCancel();
        return;
      }
      if (event.key !== "Enter" || converting) return;

      const target = event.target as HTMLElement | null;
      if (target?.tagName === "BUTTON") return;
      event.preventDefault();
      void handleConfirm();
    };

    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [converting, handleConfirm, onCancel]);

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="session-view-conversion-dialog-title"
      data-testid="session-view-conversion-dialog"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 animate-fade-in"
      onClick={() => {
        if (!converting) onCancel();
      }}
    >
      <div
        className="w-[420px] max-w-[90vw] rounded-lg border border-surface-700/50 bg-surface-800 shadow-2xl animate-slide-up"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="border-b border-surface-700 px-5 py-4">
          <h2 id="session-view-conversion-dialog-title" className="text-sm font-semibold text-status-error">
            Convert to terminal?
          </h2>
        </div>

        <div className="space-y-3 px-5 py-4 text-[13px] text-text-secondary">
          <p>
            Convert <span className="font-mono text-text-primary break-all">{sessionTitle}</span> to a terminal session?
          </p>
          <p>The structured transcript will be deleted.</p>
          <p>The previous terminal is not restored. A new terminal starts fresh.</p>
        </div>

        <div className="flex justify-end gap-3 border-t border-surface-700 px-5 py-3">
          <button
            onClick={onCancel}
            disabled={converting}
            className="cursor-pointer rounded-md px-3 py-1.5 text-sm text-text-secondary transition-colors hover:bg-surface-700/50 hover:text-text-primary disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            ref={confirmButtonRef}
            onClick={() => void handleConfirm()}
            disabled={converting}
            className="flex cursor-pointer items-center gap-2 rounded-md bg-status-error/90 px-3 py-1.5 text-sm text-white transition-colors hover:bg-status-error disabled:opacity-50"
          >
            {converting ? "Converting..." : "Convert to terminal"}
          </button>
        </div>
      </div>
    </div>
  );
}
