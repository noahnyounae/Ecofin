import type { ReactNode } from "react";

interface Props {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
}

/**
 * Panneau latéral droit, fermable.
 *
 * Générique à dessein : la catégorisation et les portefeuilles s'ouvriront
 * dans le même conteneur, avec leur propre contenu.
 */
export function SidePanel({ open, title, onClose, children }: Props) {
  return (
    <aside
      className={`side-panel${open ? " side-panel--open" : ""}`}
      aria-hidden={!open}
      // Hors écran, le panneau ne doit pas capter le focus au clavier.
      inert={!open ? true : undefined}
    >
      <header className="side-panel__header">
        <h2 className="side-panel__title">{title}</h2>
        <button
          type="button"
          className="side-panel__close"
          onClick={onClose}
          aria-label="Fermer le panneau"
        >
          ✕
        </button>
      </header>
      <div className="side-panel__body">{children}</div>
    </aside>
  );
}
