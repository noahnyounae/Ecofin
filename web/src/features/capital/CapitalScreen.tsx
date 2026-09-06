import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Unauthenticated,
  fetchAccounts,
  fetchCapital,
  fetchCategories,
  fetchGaps,
  type Period,
} from "../../api/client";
import type { Account, CapitalCurve, CategoryInfo, FogGap } from "../../api/types";
import { SidePanel } from "../../components/SidePanel";
import { date, money, moneyFromCents, plural, totals } from "../../format";
import { CapitalChart } from "./CapitalChart";
import { SubscriptionsDrawer } from "../subscriptions/SubscriptionsDrawer";
import { SearchBar } from "./SearchBar";
import { buildIndex, filterPoints, type Selection } from "./search";
import { buildTree } from "../categories/tree";
import { PeriodPicker } from "./PeriodPicker";
import { RulesScreen } from "../categories/RulesScreen";
import { WalletsScreen } from "../wallets/WalletsScreen";
import { DayPanel } from "./DayPanel";

/** Écran principal : capital dans le temps, et détail d'une opération. */
export function CapitalScreen() {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [accountId, setAccountId] = useState<string | null>(null);
  const [period, setPeriod] = useState<Period>({ from: null, to: null });
  const [curve, setCurve] = useState<CapitalCurve | null>(null);
  // La journée cliquée, et non l'opération : toutes celles d'un même jour
  // partagent la même abscisse sur le graphe et seraient sinon inatteignables.
  const [selectedDay, setSelectedDay] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [gaps, setGaps] = useState<FogGap[]>([]);
  const [showRules, setShowRules] = useState(false);
  const [showWallets, setShowWallets] = useState(false);
  // Incrémenté quand une règle change : la courbe doit être rechargée pour
  // que les catégories suivent.
  const [ruleVersion, setRuleVersion] = useState(0);
  const [query, setQuery] = useState("");
  // Non nul quand une proposition a été choisie : le filtre est alors exact,
  // qu'il porte sur un bénéficiaire ou sur un secteur.
  const [exact, setExact] = useState<Selection | null>(null);
  const [categories, setCategories] = useState<CategoryInfo[]>([]);

  useEffect(() => {
    const controller = new AbortController();
    fetchAccounts(controller.signal)
      .then((found) => {
        setAccounts(found);
        setAccountId((current) => current ?? found[0]?.id ?? null);
      })
      .catch((err: Error) => {
        // Une session expirée en cours d'usage : un rechargement ramène à
        // l'écran de connexion plutôt que d'afficher une erreur opaque.
        if (err instanceof Unauthenticated) return window.location.reload();
        if (err.name !== "AbortError") setError(err.message);
      })
      .finally(() => setLoading(false));
    return () => controller.abort();
  }, []);

  useEffect(() => {
    if (!accountId) return;
    const controller = new AbortController();
    setError(null);

    fetchCapital(accountId, period, controller.signal)
      .then(setCurve)
      .catch((err: Error) => {
        if (err instanceof Unauthenticated) return window.location.reload();
        if (err.name !== "AbortError") setError(err.message);
      });

    return () => controller.abort();
  }, [accountId, period, ruleVersion]);

  // L'arbre des secteurs entre dans l'index : taper « restauration » atteint
  // les opérations du secteur, et taper « transport » atteint aussi celles de
  // ses sous-catégories.
  const tree = useMemo(() => buildTree(categories), [categories]);

  const index = useMemo(
    () => buildIndex(curve?.points ?? [], tree),
    [curve, tree],
  );

  // Le filtre suit la frappe avec le même retard que les propositions : sans
  // cela, le graphe se redessinerait à chaque caractère.
  const [applied, setApplied] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => setApplied(query), 120);
    return () => clearTimeout(timer);
  }, [query]);

  const visible = useMemo(
    () => (curve ? filterPoints(curve.points, index, applied, exact) : []),
    [curve, index, applied, exact],
  );

  // Les totaux portent sur ce qui est affiché, et non sur toute la période :
  // une recherche qui filtre le graphe sans filtrer les sommes donnerait deux
  // lectures contradictoires du même écran.
  const summary = useMemo(
    () => totals(visible.map((point) => point.amount)),
    [visible],
  );

  const filtering = applied.trim().length > 0 || exact !== null;

  const search = useCallback((next: string, picked: Selection | null) => {
    setQuery(next);
    setExact(picked);
    // La journée retenue peut sortir du filtre : le panneau doit suivre.
    setSelectedDay(null);
  }, []);

  // Les périodes hors de portée de l'API : l'utilisateur doit savoir que sa
  // courbe comporte un trou, sans quoi il la lirait comme complète.
  useEffect(() => {
    const controller = new AbortController();
    fetchCategories(controller.signal)
      .then(setCategories)
      .catch(() => setCategories([]));
    return () => controller.abort();
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    fetchGaps(controller.signal)
      .then(setGaps)
      .catch(() => setGaps([]));
    return () => controller.abort();
  }, [accountId]);

  // Toutes les opérations de la journée retenue, dans l'ordre où elles
  // apparaissent sur la courbe.
  const dayPoints = useMemo(
    () => (selectedDay ? visible.filter((p) => p.date === selectedDay) : []),
    [visible, selectedDay],
  );

  const closePanel = useCallback(() => setSelectedDay(null), []);

  // Échap ferme le panneau : c'est le réflexe attendu d'un tiroir.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") closePanel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [closePanel]);

  const account = accounts.find((a) => a.id === accountId) ?? null;

  if (showRules) return <RulesScreen onClose={() => { setShowRules(false); setRuleVersion((v) => v + 1); }} />;
  if (loading) return <p className="screen__notice">Chargement…</p>;
  if (accounts.length === 0) {
    return (
      <p className="screen__notice">
        Aucun compte. Connecte une banque avec <code>ecofin link new</code>, puis
        lance <code>ecofin sync</code>.
      </p>
    );
  }

  return (
    <div className={`screen${dayPoints.length > 0 ? " screen--panelled" : ""}`}>
      {/* Cliquer un abonnement isole ses échéances : le dépliant réutilise le
          même filtre exact que les propositions de recherche. */}
      <SubscriptionsDrawer
        accountId={accountId}
        selected={exact?.kind === "label" ? exact.value : null}
        onSelect={(label) =>
          search(label ?? "", label ? { kind: "label", value: label } : null)
        }
      />
      <header className="screen__header">
        <div className="screen__title">
          <h1>Capital</h1>
          {account && (
            <p className="screen__subtitle">
              {account.institution} · {account.name}
              {curve && !curve.relative && ` · ${money(curve.reference_balance, curve.currency)}`}
            </p>
          )}
        </div>

        <button type="button" className="screen__rules" onClick={() => setShowRules(true)}>
          Règles de classement
        </button>

        <button
          type="button"
          className="screen__rules"
          onClick={() => setShowWallets((open) => !open)}
        >
          {showWallets ? "Masquer les portefeuilles" : "Portefeuilles"}
        </button>

        {accounts.length > 1 && (
          <select
            className="screen__account"
            value={accountId ?? ""}
            onChange={(event) => {
              setAccountId(event.target.value);
              setSelectedDay(null);
            }}
          >
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {a.institution} — {a.name}
              </option>
            ))}
          </select>
        )}
      </header>

      <div className="screen__toolbar">
        <div className="screen__filters">
          <PeriodPicker
            period={period}
            onChange={setPeriod}
            bounds={{
              first: account?.first_transaction ?? null,
              last: account?.last_transaction ?? null,
            }}
          />
          {curve && (
            <SearchBar
              points={curve.points}
              index={index}
              query={query}
              exact={exact}
              onChange={search}
            />
          )}
        </div>
        {curve && (
          <dl className="summary">
            <Stat label="Entrées" value={moneyFromCents(summary.credits, curve.currency)} />
            <Stat label="Sorties" value={moneyFromCents(summary.debits, curve.currency)} />
            <Stat label="Net" value={moneyFromCents(summary.net, curve.currency)} />
            <Stat
              label={filtering ? "Opérations filtrées" : "Opérations"}
              value={plural(summary.count, "opération")}
            />
          </dl>
        )}
      </div>

      {error && <p className="screen__error">{error}</p>}

      {gaps.length > 0 && (
        <div className="fog">
          <strong className="fog__title">
            {gaps.length > 1
              ? `${gaps.length} périodes manquantes`
              : "Période manquante"}
          </strong>
          <p className="fog__body">
            La banque n'expose que 90 jours d'historique. Ces opérations sont
            hors de sa portée et manquent à la courbe :
          </p>
          <ul className="fog__list">
            {gaps.map((gap) => (
              <li key={`${gap.account_id}-${gap.from}`}>
                du {date(gap.from)} au {date(gap.to)}
              </li>
            ))}
          </ul>
          <p className="fog__body">
            Elles figurent sur tes relevés mensuels : <code>ecofin import</code>{" "}
            les récupère.
          </p>
        </div>
      )}

      {showWallets && account && (
        <WalletsScreen
          accountId={account.id}
          currency={account.currency}
          categories={categories}
        />
      )}

      {curve && (
        <>
          {curve.relative && (
            <p className="screen__warning">
              Aucun solde bancaire connu : la courbe part de zéro. Sa forme est
              juste, ses valeurs absolues non. Lance <code>ecofin sync</code>.
            </p>
          )}
          {filtering && (
            <p className="screen__filtered">
              {visible.length === 0
                ? "Aucune opération ne correspond."
                : `${plural(visible.length, "opération")} retenue${
                    visible.length > 1 ? "s" : ""
                  } sur ${curve.points.length} — les totaux ci-dessus ne portent que sur celles-ci.`}
            </p>
          )}
          <CapitalChart
            points={visible}
            currency={curve.currency}
            selectedDay={selectedDay}
            tree={tree}
            onSelect={(point) => setSelectedDay(point.date)}
          />
        </>
      )}

      <SidePanel
        open={dayPoints.length > 0}
        title={selectedDay ? date(selectedDay) : ""}
        onClose={closePanel}
      >
        <DayPanel points={dayPoints} onRuleChanged={() => setRuleVersion((v) => v + 1)} />
      </SidePanel>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="summary__stat">
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}
