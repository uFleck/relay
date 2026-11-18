import { Checkbox as BaseCheckbox } from "@base-ui/react/checkbox";
import { Select as BaseSelect } from "@base-ui/react/select";
import { Switch as BaseSwitch } from "@base-ui/react/switch";
import type { ReactNode } from "react";

/* Relay's form controls: Base UI parts for behaviour and a11y, styled by `styles/controls.css` through the
   data attributes Base UI sets (data-checked, data-disabled, data-highlighted, data-popup-open). */

interface ToggleProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  children?: ReactNode;
  className?: string;
}

export function Checkbox({ checked, onChange, disabled, children, className }: ToggleProps) {
  return (
    <label className={`ui-check${className ? ` ${className}` : ""}`} data-disabled={disabled || undefined}>
      <BaseCheckbox.Root className="ui-box" checked={checked} disabled={disabled} onCheckedChange={(next) => onChange(next)}>
        <BaseCheckbox.Indicator className="ui-tick" />
      </BaseCheckbox.Root>
      {children}
    </label>
  );
}

export function Switch({ checked, onChange, disabled, children, className }: ToggleProps) {
  return (
    <label className={`ui-check${className ? ` ${className}` : ""}`} data-disabled={disabled || undefined}>
      {children}
      <BaseSwitch.Root className="ui-switch" checked={checked} disabled={disabled} onCheckedChange={(next) => onChange(next)}>
        <BaseSwitch.Thumb className="ui-thumb" />
      </BaseSwitch.Root>
    </label>
  );
}

export interface SelectOption<V extends string> {
  value: V;
  label: string;
}

interface SelectProps<V extends string> {
  value: V;
  options: SelectOption<V>[];
  onChange: (value: V) => void;
  /** Lets a `<label htmlFor>` name the trigger. */
  id?: string;
  disabled?: boolean;
  className?: string;
}

export function Select<V extends string>({ value, options, onChange, id, disabled, className }: SelectProps<V>) {
  return (
    <BaseSelect.Root items={options} value={value} disabled={disabled} onValueChange={(next) => next !== null && onChange(next as V)}>
      <BaseSelect.Trigger id={id} className={`ui-select${className ? ` ${className}` : ""}`}>
        <BaseSelect.Value className="ui-select-v" />
        <BaseSelect.Icon className="ui-select-i" />
      </BaseSelect.Trigger>
      <BaseSelect.Portal>
        <BaseSelect.Positioner className="ui-pos" sideOffset={4} alignItemWithTrigger={false}>
          <BaseSelect.Popup className="ui-pop">
            <BaseSelect.List>
              {options.map((o) => (
                <BaseSelect.Item key={o.value} value={o.value} className="ui-opt">
                  <BaseSelect.ItemText>{o.label}</BaseSelect.ItemText>
                  <BaseSelect.ItemIndicator className="ui-opt-i" />
                </BaseSelect.Item>
              ))}
            </BaseSelect.List>
          </BaseSelect.Popup>
        </BaseSelect.Positioner>
      </BaseSelect.Portal>
    </BaseSelect.Root>
  );
}
