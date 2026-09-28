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
        <legend className="mb-1 text-xs font-medium text-zinc-600 dark:text-zinc-400">Appearance</legend>
        <div className="grid grid-cols-3 gap-2">
          {OPTIONS.map(({ value, label, icon: Icon }) => (
            <button
              key={value}
              type="button"
              aria-pressed={theme === value}
              onClick={() => onThemeChange(value)}
              className={clsx(
                "flex flex-col items-center gap-1.5 rounded-md border px-3 py-2.5 text-sm",
                theme === value
                  ? "border-blue-500 bg-blue-50 text-blue-700 dark:border-blue-500 dark:bg-blue-950/40 dark:text-blue-300"
                  : "border-zinc-300 dark:border-zinc-700 hover:bg-zinc-100 dark:hover:bg-zinc-900",
              )}
            >
              <Icon size={18} />
              {label}
            </button>
          ))}
        </div>
      </fieldset>
      <p className="mt-3 text-xs text-zinc-500">Applies to this browser only. "System" follows your OS setting and updates automatically.</p>
    </Modal>
  );
}
