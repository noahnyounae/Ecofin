import { useMemo, useState, type FormEvent } from "react";
import {
  addCategory,
  removeCategory,
  renameCategory,
  setCategoryParent,
} from "../../api/client";
import type { CategoryInfo } from "../../api/types";
import { buildTree, flatten, pathOf, possibleParents } from "./tree";

interface Props {
  categories: CategoryInfo[];
  onChanged: () => void;
}

/**
 * Gestion des catégories : en créer, les renommer, les retirer.
 *
 * Les catégories d'origine sont désignées par 158 motifs automatiques. Les
 * supprimer sèchement ferait taire ces motifs sans que rien ne le signale :
 * leur retrait exige donc de désigner une catégorie d'accueil, qui hérite de
 * leurs motifs. Les catégories créées ici n'en ont aucun, et partent sans
 * cérémonie — avec les règles qui les désignaient.
 *
 * Une catégorie peut relever d'une autre : « Essence » sous « Transport ».
 * Chercher ou totaliser le parent embrasse alors toute sa branche, sans que
 * les opérations perdent leur classement précis.
 */
export function CategoryManager({ categories, onChanged }: Props) {
  const [label, setLabel] = useState("");
  const [isSpending, setIsSpending] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [removing, setRemoving] = useState<CategoryInfo | null>(null);
  const [into, setInto] = useState("");
  const [parent, setParent] = useState("");

  const tree = useMemo(() => buildTree(categories), [categories]);
  const ordered = useMemo(() => flatten(tree), [tree]);
  const byKey = useMemo(
    () => new Map(categories.map((c) => [c.key, c])),
    [categories],
  );

  const run = async (action: () => Promise<unknown>) => {
    setError(null);
    try {
      await action();
      onChanged();
      return true;
    } catch (err) {
      setError(err instanceof Error ? err.message : "opération refusée");
      return false;
    }
  };

  const create = async (event: FormEvent) => {
    event.preventDefault();
    if (!label.trim()) return;
    if (await run(() => addCategory(label, isSpending, parent || null))) {
      setLabel("");
    }
  };

  const startRemoval = (category: CategoryInfo) => {
    setError(null);
    if (!category.builtin) {
      void run(() => removeCategory(category.key));
      return;
    }
    // Une catégorie d'origine ne part pas sans successeur.
    setRemoving(category);
    setInto(categories.find((c) => c.key !== category.key)?.key ?? "");
  };

  const confirmRemoval = async () => {
    if (!removing) return;
    if (await run(() => removeCategory(removing.key, into))) setRemoving(null);
  };

  const rename = (category: CategoryInfo, next: string) => {
    if (!next.trim() || next === category.label) return;
    void run(() => renameCategory(category.key, next));
  };

  const reparent = (category: CategoryInfo, next: string) => {
    if ((category.parent ?? "") === next) return;
    void run(() => setCategoryParent(category.key, next || null));
  };

  return (
    <section className="manager">
      <h2 className="manager__title">Catégories</h2>

      {error && <p className="manager__error">{error}</p>}

      <ul className="manager__list">
        {ordered.map(({ key, depth }) => {
          const category = byKey.get(key);
          if (!category) return null;
          return (
          <li
            key={category.key}
            className="manager__item"
            style={{ marginLeft: `${depth * 1.25}rem` }}
          >
            <input
              className="manager__name"
              defaultValue={category.label}
              aria-label={`Nom de ${category.label}`}
              onBlur={(event) => rename(category, event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") event.currentTarget.blur();
              }}
            />
            <select
              className="manager__parent"
              value={category.parent ?? ""}
              aria-label={`Catégorie parente de ${category.label}`}
              onChange={(event) => reparent(category, event.target.value)}
            >
              <option value="">— à la racine —</option>
              {possibleParents(category.key, tree).map((candidate) => (
                <option key={candidate} value={candidate}>
                  {pathOf(candidate, tree)}
                </option>
              ))}
            </select>
            {!category.is_spending && (
              <span className="manager__flag" title="N'est pas comptée dans les dépenses">
                hors dépenses
              </span>
            )}
            {!category.builtin && <span className="manager__flag">à toi</span>}
            <button
              type="button"
              className="manager__remove"
              onClick={() => startRemoval(category)}
              title={
                category.builtin
                  ? "Retirer, en redirigeant ses motifs automatiques"
                  : "Supprimer, avec les règles qui la désignent"
              }
            >
              ✕
            </button>
          </li>
          );
        })}
      </ul>

      {removing && (
        <div className="manager__confirm">
          <p>
            <strong>{removing.label}</strong> est une catégorie d'origine : des
            motifs automatiques la désignent. Choisis celle qui doit les
            recevoir — sans quoi ces motifs cesseraient de classer.
          </p>
          <div className="manager__confirm-row">
            <select value={into} onChange={(event) => setInto(event.target.value)}>
              {categories
                .filter((c) => c.key !== removing.key)
                .map((c) => (
                  <option key={c.key} value={c.key}>
                    {pathOf(c.key, tree)}
                  </option>
                ))}
            </select>
            <button type="button" onClick={() => void confirmRemoval()}>
              Rediriger et retirer
            </button>
            <button type="button" onClick={() => setRemoving(null)}>
              Annuler
            </button>
          </div>
        </div>
      )}

      <form className="manager__add" onSubmit={create}>
        <input
          value={label}
          onChange={(event) => setLabel(event.target.value)}
          placeholder="Nouvelle catégorie"
          aria-label="Nom de la nouvelle catégorie"
        />
        <select
          value={parent}
          aria-label="Catégorie parente de la nouvelle"
          onChange={(event) => setParent(event.target.value)}
        >
          <option value="">— à la racine —</option>
          {ordered.map(({ key }) => (
            <option key={key} value={key}>
              {pathOf(key, tree)}
            </option>
          ))}
        </select>
        <label className="manager__spending">
          <input
            type="checkbox"
            checked={isSpending}
            onChange={(event) => setIsSpending(event.target.checked)}
          />
          Compte comme une dépense
        </label>
        <button type="submit" disabled={!label.trim()}>
          Ajouter
        </button>
      </form>

      <p className="manager__hint">
        Une catégorie créée ici n'est jamais attribuée automatiquement : elle ne
        s'obtient qu'en classant une opération depuis son inspecteur. Rangée
        sous une autre, ses opérations comptent aussi dans celle-ci : chercher
        « Transport » donne Transport, Essence et Péage réunis.
      </p>
    </section>
  );
}
