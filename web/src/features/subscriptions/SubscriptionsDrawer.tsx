import { useEffect, useState } from "react";
import { fetchSubscriptions } from "../../api/client";
import type { Subscription } from "../../api/types";
import { date, money, moneyFromCents, toCents } from "../../format";

/** Rythmes, en français. */
const CADENCE: Record<Subscription["cadence"], string> = {
  weekly: "hebdomadaire",
  monthly: "mensuel",
  quarterly: "trimestriel",
  yearly: "annuel",
};

interface Props {
  accountId: string | null;
  /** Libellé actuellement isolé sur le graphe, s'il y en a un. */
  selected: string | null;
  onSelect: (label: string | null) => void;
}

/**
 * Dépliant gauche : les prélèvements récurrents détectés.
 *
 * Cliquer un abonnement isole ses échéances sur le graphe — c'est la façon la
 * plus directe de vérifier qu'une détection est juste, et de voir à quel
 * moment un montant a changé.
 */
export function SubscriptionsDrawer({ accountId, selected, onSelect }: Props) {
  const [open, setOpen] = useState(false);
  const [subscriptions, setSubscriptions] = useState<Subscription[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!accountId) return;
    const controller = new AbortController();
    setError(null);

    fetchSubscriptions(accountId, controller.signal)
      .then(setSubscriptions)
      .catch((err: Error) => {
        if (err.name !== "AbortError") setError(err.message);
      });

    return () => controller.abort();
  }, [accountId]);

  const active = subscriptions.filter((s) => s.active);
  const stopped = subscriptions.filter((s) => !s.active);

  // Les rythmes sont ramenés au mois côté serveur : ils s'additionnent donc.
  const monthly = active.reduce((sum, s) => sum + toCents(s.monthly_cost), 0);

  return (
    <>
      <button
        type="button"
        className={`drawer-toggle${open ? " drawer-toggle--open" : ""}`}
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
      >
        <span className="drawer-toggle__label">Abonnements</span>
        {active.length > 0 && (
          <span className="drawer-toggle__badge">{active.length}</span>
        )}
      </button>

      <aside
        className={`drawer${open ? " drawer--open" : ""}`}
        aria-hidden={!open}
        inert={!open ? true : undefined}
      >
        <header className="drawer__header">
          <h2 className="drawer__title">Abonnements détectés</h2>
          <button
            type="button"
            className="drawer__close"
            onClick={() => setOpen(false)}
            aria-label="Replier le panneau"
          >
            ✕
          </button>
        </header>

        <div className="drawer__body">
          {error && <p className="drawer__error">{error}</p>}

          {!error && subscriptions.length === 0 && (
            <p className="drawer__empty">
              Aucun prélèvement régulier repéré. La détection demande au moins
              trois échéances à intervalle constant.
            </p>
          )}

          {active.length > 0 && (
            <>
              <div className="drawer__total">
                <span className="drawer__total-label">Charge mensuelle</span>
                <span className="drawer__total-value">{moneyFromCents(monthly)}</span>
              </div>
              <Section
                title="En cours"
                items={active}
                selected={selected}
                onSelect={onSelect}
              />
            </>
          )}

          {stopped.length > 0 && (
            <Section
              title="Arrêtés"
              hint="Plus aucune échéance depuis deux périodes."
              items={stopped}
              selected={selected}
              onSelect={onSelect}
            />
          )}
        </div>
      </aside>
    </>
  );
}

function Section({
  title,
  hint,
  items,
  selected,
  onSelect,
}: {
  title: string;
  hint?: string;
  items: Subscription[];
  selected: string | null;
  onSelect: (label: string | null) => void;
}) {
  return (
    <section className="drawer__section">
      <h3 className="drawer__section-title">
        {title}
        <span className="drawer__section-count">{items.length}</span>
      </h3>
      {hint && <p className="drawer__section-hint">{hint}</p>}

      <ul className="subs">
        {items.map((item) => {
          const isSelected = selected === item.label;
          return (
            <li key={item.label}>
              <button
                type="button"
                className={`sub${isSelected ? " sub--selected" : ""}`}
                // Recliquer le même abonnement lève le filtre : le bouton fait
                // office d'interrupteur.
                onClick={() => onSelect(isSelected ? null : item.label)}
              >
                <span className="sub__head">
                  <span className="sub__label">{item.label}</span>
                  <span className="sub__amount">
                    {money(item.amount)}
                    {item.variable_amount && (
                      <span className="sub__variable" title="Montant variable d'une échéance à l'autre">
                        ~
                      </span>
                    )}
                  </span>
                </span>
                <span className="sub__meta">
                  {CADENCE[item.cadence]} · {item.occurrences} échéances · dernière{" "}
                  {date(item.last_seen)}
                  {item.cadence !== "monthly" && (
                    <> · {moneyFromCents(toCents(item.monthly_cost))}/mois</>
                  )}
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
