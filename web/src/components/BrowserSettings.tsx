import clsx from "clsx";
import { Monitor, Moon, Sun } from "lucide-react";
import type { ThemePreference } from "../theme";
import { Modal } from "./ui";

const OPTIONS: { value: ThemePreference; label: string; icon: typeof Sun }[] = [
  { value: "system", label: "System", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
];

export function BrowserSettingsModal({
  open,
  onClose,
  theme,
  onThemeChange,
}: {
  open: boolean;
  onClose: () => void;
  theme: ThemePreference;
  onThemeChange: (pref: ThemePreference) => void;
}) {
  return (
    <Modal open={open} onClose={onClose} title="Browser settings">
      <fieldset className="space-y-2">
        <legend className="mb-2 text-xs font-medium text-fg-muted">Appearance</legend>
        <div className="grid grid-cols-3 gap-2.5">
          {OPTIONS.map(({ value, label, icon: Icon }) => (
            <button
              key={value}
              type="button"
              aria-pressed={theme === value}
              onClick={() => onThemeChange(value)}
              className={clsx(
                "flex flex-col items-center gap-2 rounded-xl border px-3 py-4 text-sm font-medium transition-colors",
                theme === value ? "border-accent/50 bg-accent-soft text-accent-fg ring-1 ring-accent/30" : "border-line text-fg-muted hover:bg-surface-2 hover:text-fg",
              )}
            >
              <Icon size={20} />
              {label}
            </button>
          ))}
        </div>
      </fieldset>
      <p className="mt-4 text-xs text-fg-subtle">Applies to this browser only. "System" follows your OS setting and updates automatically.</p>
    </Modal>
  );
}
