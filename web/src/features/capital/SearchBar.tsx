import { useEffect, useMemo, useRef, useState } from "react";
import type { CapitalPoint } from "../../api/types";
import { plural } from "../../format";
import { suggest, tolerance, type SearchIndex, type Selection, type Suggestion } from "./search";

interface Props {
  points: CapitalPoint[];
  /** Index préparé par l'écran, qui connaît aussi les secteurs. */
  index: SearchIndex;
  query: string;
  exact: Selection | null;
  onChange: (query: string, exact: Selection | null) => void;
}

/**
 * Recherche dans les libellés, avec propositions au fil de la frappe.
 *
 * La correspondance porte sur le libellé complet et tolère les fautes ; les
 * propositions, elles, affichent le libellé nettoyé, seul lisible.
 */
export function SearchBar({ points, index, query, exact, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const [highlighted, setHighlighted] = useState(0);
  const container = useRef<HTMLDivElement>(null);

  // La recherche approximative coûte quelques dizaines de millisecondes sur un
  // millier de libellés. Attendre une pause dans la frappe évite de la relancer
  // à chaque caractère, pour un résultat aussitôt périmé.
  const [debounced, setDebounced] = useState(query);
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(query), 120);
    return () => clearTimeout(timer);
  }, [query]);

  const suggestions = useMemo(
    () => (debounced.trim() ? suggest(points, index, debounced) : []),
    [points, index, debounced],
  );

  useEffect(() => setHighlighted(0), [debounced]);

  // Un clic hors du champ referme la liste sans annuler la recherche.
  useEffect(() => {
    const onClickOutside = (event: MouseEvent) => {
      if (!container.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onClickOutside);
    return () => document.removeEventListener("mousedown", onClickOutside);
  }, []);

  const choose = (suggestion: Suggestion) => {
    onChange(suggestion.label, { kind: suggestion.kind, value: suggestion.value });
    setOpen(false);
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "Escape") {
      onChange("", null);
      setOpen(false);
      return;
    }
    if (!open || suggestions.length === 0) return;

    if (event.key === "ArrowDown") {
      event.preventDefault();
      setHighlighted((i) => (i + 1) % suggestions.length);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setHighlighted((i) => (i - 1 + suggestions.length) % suggestions.length);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const picked = suggestions[highlighted];
      if (picked) choose(picked);
    }
  };

  const approximate = tolerance(query.trim().length) > 0;

  return (
    <div className="search" ref={container}>
      <label className="search__label" htmlFor="search-input">
        Rechercher
      </label>
      <div className="search__field">
        <input
          id="search-input"
          type="search"
          value={query}
          placeholder="Commerçant, libellé…"
          autoComplete="off"
          onFocus={() => setOpen(true)}
          onKeyDown={onKeyDown}
          onChange={(event) => {
            // Toute frappe abandonne la sélection exacte : on repasse en
            // recherche libre.
            onChange(event.target.value, null);
            setOpen(true);
          }}
        />
        {(query || exact) && (
          <button
            type="button"
            className="search__clear"
            onClick={() => onChange("", null)}
            aria-label="Effacer la recherche"
          >
            ✕
          </button>
        )}
      </div>

      {open && suggestions.length > 0 && (
        <ul className="search__suggestions">
          {suggestions.map((suggestion, index) => (
            <li key={`${suggestion.kind}-${suggestion.value}`}>
              <button
                type="button"
                className={`search__suggestion${
                  index === highlighted ? " search__suggestion--active" : ""
                }`}
                onMouseEnter={() => setHighlighted(index)}
                onClick={() => choose(suggestion)}
              >
                <span className="search__suggestion-label">
                  {suggestion.kind === "category" && (
                    <span className="search__kind">secteur</span>
                  )}
                  {suggestion.label}
                </span>
                <span className="search__suggestion-count">
                  {plural(suggestion.count, "opération")}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}

      {query.trim() && !exact && (
        <p className="search__hint">
          {approximate
            ? "Recherche dans le libellé complet, fautes de frappe tolérées."
            : "Recherche exacte : ajoute un caractère pour tolérer les fautes."}
        </p>
      )}
    </div>
  );
}
