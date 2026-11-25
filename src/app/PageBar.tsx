import { createContext, useContext, type ReactNode, type Ref } from "react";
import { createPortal } from "react-dom";

/* The title bar is part of the page: each view renders its title, meta line and actions
   into the bar's slots, so they sit beside the window controls instead of in a second row. */

export interface PageBarSlots {
  title: HTMLElement | null;
  actions: HTMLElement | null;
}

export const PageBarContext = createContext<PageBarSlots>({ title: null, actions: null });

interface Props {
  title: string;
  meta?: ReactNode;
  actions?: ReactNode;
  headingRef?: Ref<HTMLHeadingElement>;
}

export function PageBar({ title, meta, actions, headingRef }: Props) {
  const slots = useContext(PageBarContext);
  return (
    <>
      {slots.title &&
        createPortal(
          <>
            <h1 ref={headingRef} tabIndex={-1} data-tauri-drag-region>
              {title}
            </h1>
            {meta && (
              <span className="tb-meta meta" data-tauri-drag-region>
                {meta}
              </span>
            )}
          </>,
          slots.title,
        )}
      {slots.actions && actions && createPortal(actions, slots.actions)}
    </>
  );
}
