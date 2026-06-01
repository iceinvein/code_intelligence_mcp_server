import type { SettingField as Field, SettingValue } from "@/api/settings";

type Props = {
  field: Field;
  draftValue: SettingValue | undefined;
  dirty: boolean;
  onChange: (key: string, value: SettingValue) => void;
  onReset: (key: string) => void;
};

export function SettingField({ field, draftValue, dirty, onChange, onReset }: Props) {
  const current = draftValue !== undefined ? draftValue : field.value;
  const disabled = !field.editable;

  return (
    <div className="flex items-start justify-between gap-4 border-b border-border py-2">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2">
          <span className="font-mono text-[12px] text-foreground">{field.key}</span>
          {field.needs_reindex ? (
            <span className="text-[9px] uppercase tracking-wide text-amber-500">reindex</span>
          ) : null}
          {dirty ? (
            <span className="text-[9px] uppercase tracking-wide text-primary">edited</span>
          ) : null}
        </div>
        <div className="mt-0.5 text-[11px] text-muted-foreground">{field.description}</div>
      </div>

      <div className="flex shrink-0 items-center gap-2">
        {field.type === "bool" ? (
          <select
            className="h-7 rounded-md border border-border bg-background px-2 py-1 text-xs"
            disabled={disabled}
            value={String(current)}
            onChange={(e) => onChange(field.key, e.target.value === "true")}
          >
            <option value="true">on</option>
            <option value="false">off</option>
          </select>
        ) : field.type === "enum" ? (
          <select
            className="h-7 rounded-md border border-border bg-background px-2 py-1 text-xs"
            disabled={disabled}
            value={String(current)}
            onChange={(e) => onChange(field.key, e.target.value)}
          >
            {(field.options ?? []).map((o) => (
              <option key={o} value={o}>
                {o}
              </option>
            ))}
          </select>
        ) : field.type === "number" ? (
          <input
            type="number"
            className="h-7 w-28 rounded-md border border-border bg-background px-2 py-1 text-right font-mono text-xs"
            disabled={disabled}
            value={String(current)}
            min={field.range?.min}
            max={field.range?.max}
            step="any"
            onChange={(e) => onChange(field.key, Number(e.target.value))}
          />
        ) : (
          <input
            type="text"
            className="h-7 w-72 max-w-[42vw] rounded-md border border-border bg-background px-2 py-1 font-mono text-xs"
            disabled={disabled}
            value={String(current)}
            onChange={(e) => onChange(field.key, e.target.value)}
          />
        )}
        {field.editable ? (
          <button
            type="button"
            className="text-[11px] text-muted-foreground hover:text-foreground disabled:opacity-30"
            disabled={current === field.default}
            title="reset to default"
            onClick={() => onReset(field.key)}
          >
            reset
          </button>
        ) : null}
      </div>
    </div>
  );
}
