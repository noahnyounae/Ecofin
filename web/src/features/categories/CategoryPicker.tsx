import { useMemo, useState } from "react";
import { deleteRule, setRule } from "../../api/client";
import type { CategoryInfo } from "../../api/types";
import { buildTree, flatten } from "./tree";

interface Props {
  /** Libellé nettoyé du bénéficiaire : c'est lui que la règle vise. */
  label: string;
  /** Catégorie actuelle, sous son nom stable. */
  category: string;
  /** Vrai si elle vient déjà d'une règle de l'utilisateur. */
  isManual: boolean;
  categories: CategoryInfo[];
  onChanged: () => void;
}

/**
 * Choix de la catégorie d'une dépense, depuis l'inspecteur.
 *
 * La règle posée vise le **bénéficiaire**, pas l'opération : classer un achat
 * chez un commerçant classe tous ses achats, passés comme à venir. C'est ce
 * qui rend le classement manuel praticable — corriger 1786 opérations une par
 * une ne le serait pas.
 */
export function CategoryPicker({ label, category, isManual, categories, onChanged }: Props) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // L'emboîtement se lit à l'indentation : « Essence » sous « Transport »
  // dit, sans phrase, que classer là compte aussi dans le secteur parent.
  const tree = useMemo(() => buildTree(categories), [categories]);
  const ordered = useMemo(() => flatten(tree), [tree]);

  const apply = async (next: string) => {
    setPending(true);
    setError(null);
    try {
      await setRule(label, next);
      onChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : "échec");
    } finally {
      setPending(false);
    }
  };

  const reset = async () => {
    setPending(true);
    setError(null);
    try {
      await deleteRule(label);
      onChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : "échec");
    } finally {
      setPending(false);
    }
  };

  return (
    <div className="picker">
      <label className="picker__label" htmlFor="category-select">
        Catégorie
      </label>
      <div className="picker__row">
        <select
          id="category-select"
          value={category}
          disabled={pending}
          onChange={(event) => void apply(event.target.value)}
        >
          {ordered.map(({ key, depth }) => (
            <option key={key} value={key}>
              {`${"\u00a0\u00a0".repeat(depth)}${depth > 0 ? "└ " : ""}${
                tree.labels.get(key) ?? key
              }`}
            </option>
          ))}
        </select>
        {isManual && (
          <button
            type="button"
            className="picker__reset"
            onClick={() => void reset()}
            disabled={pending}
            title="Revenir au classement automatique"
          >
            Auto
          </button>
        )}
      </div>

      <p className="picker__hint">
        {isManual
          ? `Classé par toi. S'applique à toutes les opérations « ${label} ».`
          : `Classé automatiquement. Un changement s'appliquera à toutes les opérations « ${label} ».`}
      </p>
      {error && <p className="picker__error">{error}</p>}
    </div>
  );
}
