import { useEffect, useState } from "react";
import { fetchCategories, fetchTransaction } from "../../api/client";
import type { CategoryInfo } from "../../api/types";
import { CategoryPicker } from "../categories/CategoryPicker";
import type { CapitalPoint, TransactionDetail } from "../../api/types";
import { date, money } from "../../format";

interface Props {
  /** Opérations de la journée cliquée, de la plus ancienne à la plus récente. */
  points: CapitalPoint[];
  /** Appelé quand une règle change : la courbe doit se reclasser. */
  onRuleChanged: () => void;
}

/**
 * Contenu du panneau latéral : les opérations d'une journée.
 *
 * Le graphe place toutes les opérations d'un même jour sur la même verticale.
 * Recharts n'y désigne le point actif que par son abscisse : la deuxième
 * opération d'une journée, et les suivantes, étaient donc hors d'atteinte —
 * 56 % des opérations sur cet historique. Le clic sélectionne désormais la
 * journée, et c'est ici qu'on choisit laquelle regarder.
 */
export function DayPanel({ points, onRuleChanged }: Props) {
  // Une seule opération : on ouvre son détail sans détour.
  const [focused, setFocused] = useState<string | null>(
    points.length === 1 ? (points[0]?.transaction_id ?? null) : null,
  );

  // Changer de journée ramène à la liste, ou au détail s'il n'y a qu'une
  // opération.
  useEffect(() => {
    setFocused(points.length === 1 ? (points[0]?.transaction_id ?? null) : null);
  }, [points]);

  if (focused) {
    return (
      <>
        {points.length > 1 && (
          <button type="button" className="panel__back" onClick={() => setFocused(null)}>
            ← {points.length} opérations ce jour-là
          </button>
        )}
        <TransactionDetailView transactionId={focused} onRuleChanged={onRuleChanged} />
      </>
    );
  }

  const total = points.reduce((sum, p) => sum + Number(p.amount), 0);

  return (
    <>
      <p className="day__summary">
        {points.length} opérations, solde net {money(total)}
      </p>
      <ul className="day__list">
        {points.map((point) => (
          <li key={point.transaction_id}>
            <button
              type="button"
              className="day__item"
              onClick={() => setFocused(point.transaction_id)}
            >
              <span className="day__item-label">{point.description}</span>
              <span
                className={`day__item-amount${
                  Number(point.amount) < 0 ? " day__item-amount--debit" : ""
                }`}
              >
                {money(point.amount, point.currency)}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </>
  );
}

function TransactionDetailView({
  transactionId,
  onRuleChanged,
}: {
  transactionId: string;
  onRuleChanged: () => void;
}) {
  const [detail, setDetail] = useState<TransactionDetail | null>(null);
  const [categories, setCategories] = useState<CategoryInfo[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Incrémenté à chaque règle posée : force la relecture du détail, dont la
  // catégorie vient de changer.
  const [version, setVersion] = useState(0);

  useEffect(() => {
    const controller = new AbortController();
    fetchCategories(controller.signal)
      .then(setCategories)
      .catch(() => setCategories([]));
    return () => controller.abort();
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    setError(null);
    setDetail(null);

    fetchTransaction(transactionId, controller.signal)
      .then(setDetail)
      .catch((err: Error) => {
        if (err.name !== "AbortError") setError(err.message);
      });

    return () => controller.abort();
  }, [transactionId, version]);

  if (error) return <p className="panel__error">{error}</p>;
  if (!detail) return <p className="panel__loading">Chargement…</p>;

  const debit = Number(detail.amount) < 0;

  return (
    <dl className="detail">
      <div className={`detail__amount${debit ? " detail__amount--debit" : ""}`}>
        {money(detail.amount, detail.currency)}
      </div>

      {categories.length > 0 && (
        <CategoryPicker
          label={detail.description}
          category={detail.category}
          isManual={detail.category_is_manual}
          categories={categories}
          onChanged={() => {
            setVersion((v) => v + 1);
            onRuleChanged();
          }}
        />
      )}

      <Row label="Libellé" value={detail.description} />
      <Row
        label="Libellé d'origine"
        value={detail.raw_description}
        monospace
        hideIfSame={detail.description}
      />
      <Row label="Contrepartie" value={detail.counterparty} />
      <Row label="Comptabilisé le" value={date(detail.booking_date)} />
      {detail.value_date && <Row label="Date de valeur" value={date(detail.value_date)} />}
      <Row label="Catégorie banque" value={detail.bank_category} />
      <Row label="État" value={detail.booked ? "Comptabilisée" : "En attente"} />
      <Row label="Provenance" value={detail.source} />
    </dl>
  );
}

function Row({
  label,
  value,
  monospace,
  hideIfSame,
}: {
  label: string;
  value: string | null;
  monospace?: boolean;
  /** Masque la ligne quand elle répéterait la précédente. */
  hideIfSame?: string;
}) {
  if (!value) return null;
  if (hideIfSame !== undefined && hideIfSame === value) return null;
  return (
    <div className="detail__row">
      <dt>{label}</dt>
      <dd className={monospace ? "detail__raw" : undefined}>{value}</dd>
    </div>
  );
}
