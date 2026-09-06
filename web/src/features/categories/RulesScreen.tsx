import { useCallback, useEffect, useState } from "react";
import { deleteRule, fetchCategories, fetchRules, setRule } from "../../api/client";
import type { CategoryInfo, CategoryRule } from "../../api/types";
import { CategoryManager } from "./CategoryManager";

/**
 * Configuration des règles de classement.
 *
 * Chaque règle associe un bénéficiaire à un secteur, et prime sur le classement
 * automatique. La liste montre ce que l'utilisateur a décidé — et rien d'autre :
 * les milliers d'opérations classées par motif interne n'y figurent pas, seules
 * les corrections.
 */
export function RulesScreen({ onClose }: { onClose: () => void }) {
  const [rules, setRules] = useState<CategoryRule[]>([]);
  const [categories, setCategories] = useState<CategoryInfo[]>([]);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(() => {
    fetchRules()
      .then(setRules)
      .catch((err: Error) => setError(err.message));
    fetchCategories()
      .then(setCategories)
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    fetchCategories(controller.signal)
      .then(setCategories)
      .catch(() => setCategories([]));
    reload();
    return () => controller.abort();
  }, [reload]);

  const change = async (label: string, category: string) => {
    await setRule(label, category);
    reload();
  };

  const remove = async (label: string) => {
    await deleteRule(label);
    reload();
  };

  const nameOf = (key: string) =>
    categories.find((c) => c.key === key)?.label ?? key;

  return (
    <div className="rules">
      <header className="rules__header">
        <div>
          <h1 className="rules__title">Règles de classement</h1>
          <p className="rules__subtitle">
            Chaque règle vise un bénéficiaire et l'emporte sur le classement
            automatique.
          </p>
        </div>
        <button type="button" className="rules__close" onClick={onClose}>
          Retour
        </button>
      </header>

      {error && <p className="rules__error">{error}</p>}

      <CategoryManager categories={categories} onChanged={reload} />

      <h2 className="rules__section">Bénéficiaires classés à la main</h2>

      {rules.length === 0 ? (
        <p className="rules__empty">
          Aucune règle. Ouvre une opération sur le graphe et choisis sa
          catégorie : la règle apparaîtra ici.
        </p>
      ) : (
        <ul className="rules__list">
          {rules.map((rule) => (
            <li key={rule.label} className="rules__item">
              <span className="rules__item-label">{rule.label}</span>
              <select
                value={rule.category}
                onChange={(event) => void change(rule.label, event.target.value)}
              >
                {categories.map((c) => (
                  <option key={c.key} value={c.key}>
                    {c.label}
                  </option>
                ))}
              </select>
              <button
                type="button"
                className="rules__remove"
                onClick={() => void remove(rule.label)}
                title={`Rendre « ${rule.label} » au classement automatique`}
              >
                ✕
              </button>
            </li>
          ))}
        </ul>
      )}

      {rules.length > 0 && (
        <p className="rules__footer">
          {rules.length} règle{rules.length > 1 ? "s" : ""} · retirer une règle
          rend le bénéficiaire au classement automatique, il ne le laisse pas
          sans catégorie. Catégories disponibles : {categories.length}, dont{" "}
          {nameOf("transfer")} et {nameOf("investment")} qui ne comptent pas
          comme des dépenses.
        </p>
      )}
    </div>
  );
}
