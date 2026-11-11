import { Check, ChevronDown } from "lucide-react";
import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { cx, useDismiss } from "./ui";

export interface Option {
  value: string;
  label: string;
  hint?: string;
  /** Shown in the warning color (e.g. full access). */
  warn?: boolean;
}

/**
 * A dropdown in the app's own style, instead of the browser's native one.
 * `pill` sits in a toolbar row; `row` fills a properties row.
 */
export function Select({
  value,
  options,
  onChange,
  label,
  icon,
  variant = "pill",
}: {
  value: string;
  options: Option[];
  onChange: (value: string) => void;
  label: string;
  icon?: ReactNode;
  variant?: "pill" | "row";
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const wrap = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const id = useId();
  const current = options.find((o) => o.value === value) ?? options[0];

  const close = () => {
    setOpen(false);
    trigger.current?.focus();
  };
  useDismiss(open, wrap, close);
  useEffect(() => {
    if (open) list.current?.focus();
  }, [open]);

  const show = () => {
    setActive(Math.max(0, options.findIndex((o) => o.value === value)));
    setOpen(true);
  };
  const pick = (o: Option) => {
    close();
    if (o.value !== value) onChange(o.value);
  };

  const onTriggerKey = (e: KeyboardEvent) => {
    if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
      e.preventDefault();
      show();
    }
  };
  const onListKey = (e: KeyboardEvent) => {
    const last = options.length - 1;
    const moves: Record<string, () => number> = {
      ArrowDown: () => Math.min(last, active + 1),
      ArrowUp: () => Math.max(0, active - 1),
      Home: () => 0,
      End: () => last,
    };
    if (moves[e.key]) {
      e.preventDefault();
      setActive(moves[e.key]());
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      pick(options[active]);
    } else if (e.key === "Tab") {
      setOpen(false);
    } else if (e.key.length === 1 && /\S/.test(e.key)) {
      // Type-ahead: the next option starting with that letter.
      const k = e.key.toLowerCase();
      const order = [...options.slice(active + 1), ...options.slice(0, active + 1)];
      const hit = order.find((o) => o.label.toLowerCase().startsWith(k));
      if (hit) setActive(options.indexOf(hit));
    }
  };

  return (
    <div ref={wrap} className={cx("relative", variant === "row" && "w-full")}>
      <button
        ref={trigger}
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={`${label}: ${current.label}`}
        onClick={() => (open ? close() : show())}
        onKeyDown={onTriggerKey}
        className={cx(
          "group flex items-center text-left transition-colors",
          variant === "pill"
            ? "h-6 gap-1 rounded-md px-1.5 text-[11.5px] ring-1 ring-inset hover:bg-panel-2"
            : "h-8 w-full gap-2.5 rounded-md px-2 text-[13px] hover:bg-panel-2",
          variant === "pill" && (current.warn ? "ring-s-needs/40" : "ring-line-strong"),
          open && "bg-panel-2",
        )}
      >
        {icon && <span className={cx("shrink-0", current.warn ? "text-s-needs" : "text-muted")}>{icon}</span>}
        <span className={cx("min-w-0 truncate", variant === "row" && "flex-1", current.warn ? "text-s-needs" : "text-fg")}>{current.label}</span>
        <ChevronDown className={cx("size-3 shrink-0 text-faint transition-opacity", variant === "row" && !open && "opacity-0 group-hover:opacity-100")} />
      </button>
      {open && (
        <div
          ref={list}
          role="listbox"
          aria-label={label}
          tabIndex={-1}
          aria-activedescendant={`${id}-${active}`}
          onKeyDown={onListKey}
          className={cx("animate-pop absolute z-50 mt-1 rounded-lg bg-panel p-1 shadow-pop outline-none", variant === "row" ? "inset-x-0" : "left-0 w-[232px]")}
        >
          {options.map((o, i) => (
            <div
              key={o.value}
              id={`${id}-${i}`}
              role="option"
              aria-selected={o.value === value}
              onMouseMove={() => setActive(i)}
              onClick={() => pick(o)}
              className={cx("flex cursor-pointer items-start gap-2 rounded-md px-2 py-1", i === active && "bg-panel-2")}
            >
              <span className="min-w-0 flex-1">
                <span className={cx("block text-[12.5px] leading-5", o.warn ? "text-s-needs" : "text-fg")}>{o.label}</span>
                {o.hint && <span className="block text-[11px] leading-snug text-faint">{o.hint}</span>}
              </span>
              {o.value === value && <Check className="mt-1 size-3 shrink-0 text-accent" />}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
