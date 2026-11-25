import { AlertDialog } from "@base-ui/react/alert-dialog";
import { useState } from "react";
import { errorText } from "../ipc";

export function DeleteConfirmation({ label, description, blocked, onDelete }: {
  label: string;
  description: string;
  blocked?: boolean;
  onDelete: () => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const remove = async () => {
    setBusy(true);
    setError(null);
    try {
      await onDelete();
      setOpen(false);
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <AlertDialog.Root open={open} onOpenChange={(next) => {
      if (busy) return;
      setError(null);
      setOpen(next);
    }}>
      <AlertDialog.Trigger className="btn ghost" data-vim-delete="">{label}</AlertDialog.Trigger>
      <AlertDialog.Portal>
        <AlertDialog.Backdrop className="delete-backdrop" />
        <AlertDialog.Popup className="delete-confirmation">
          <AlertDialog.Title>{label}</AlertDialog.Title>
          <AlertDialog.Description>{description}</AlertDialog.Description>
          {error && <p role="alert">{error}</p>}
          <div className="delete-actions">
            <AlertDialog.Close className="btn" disabled={busy}>{blocked ? "Close" : "Cancel"}</AlertDialog.Close>
            {!blocked && (
              <button type="button" className="btn delete-action" disabled={busy} onClick={() => void remove()}>
                {busy ? "Deleting…" : label}
              </button>
            )}
          </div>
        </AlertDialog.Popup>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}
