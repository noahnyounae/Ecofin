import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";
import {
  addWallet,
  fetchWallets,
  removeWallet,
  updateWallet,
} from "../../api/client";
import type { CategoryInfo, WalletInfo, WalletsState } from "../../api/types";
import { moneyFromCents } from "../../format";
import { toCents } from "../../format";
import { buildTree, flatten, pathOf } from "../categories/tree";
import { arrange, possibleParents } from "./nesting";

interface Props {
  accountId: string;
  currency: string;
  categories: CategoryInfo[];
}

/**
 * Portefeuilles virtuels : répartir le solde en enveloppes mensuelles.
 *
 * Une enveloppe n'est pas un compte — aucun argent n'y est déplacé. C'est une
 * somme qu'on décide de consacrer chaque mois à un usage, que les dépenses des
 * catégories rattachées viennent entamer, et dont le reliquat rouvre le mois
 * suivant plutôt que de s'évaporer.
 *
 * Les enveloppes s'emboîtent, et une mère **dote** ses filles : sa dotation
 * est le budget de toute la branche, et ce qu'il en reste finance ses propres
 * catégories. L'argent d'une fille est donc déjà celui de sa mère, jamais
 * compté deux fois.
 *
 * La ligne « non alloué » n'est pas un reste comptable : c'est elle qui rend
 * vraie la promesse que la somme des enveloppes vaut le solde du compte.
 */
export function WalletsScreen({ accountId, currency, categories }: Props) {
  const [state, setState] = useState<WalletsState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<number | null>(null);
  const [version, setVersion] = useState(0);

  const tree = useMemo(() => buildTree(categories), [categories]);
  const ordered = useMemo(() => flatten(tree), [tree]);

  useEffect(() => {
    if (!accountId) return;
    const controller = new AbortController();
    fetchWallets(accountId, undefined, controller.signal)
      .then(setState)
      .catch((err: unknown) => {
        if (controller.signal.aborted) return;
        setError(err instanceof Error ? err.message : "chargement impossible");
      });
    return () => controller.abort();
  }, [accountId, version]);

  const reload = useCallback(() => setVersion((v) => v + 1), []);

  const run = async (action: () => Promise<unknown>) => {
    setError(null);
    try {
      await action();
      reload();
      return true;
    } catch (err) {
      setError(err instanceof Error ? err.message : "opération refusée");
      return false;
    }
  };

  if (!state) {
    return (
      <section className="wallets">
        <h2 className="wallets__title">Portefeuilles</h2>
        {error && <p className="wallets__error">{error}</p>}
      </section>
    );
  }

  const money = (raw: string) => moneyFromCents(toCents(raw), currency);

  return (
    <section className="wallets">
      <header className="wallets__header">
        <h2 className="wallets__title">Portefeuilles</h2>
        <span className="wallets__month">{state.month}</span>
      </header>

      {error && <p className="wallets__error">{error}</p>}

      <ul className="wallets__list">
        {arrange(state.wallets).map(({ wallet, depth }) =>
          editing === wallet.id ? (
            <li
              key={wallet.id}
              className="wallets__item"
              style={{ marginLeft: `${depth * 1.4}rem` }}
            >
              <WalletForm
                wallet={wallet}
                others={state.wallets.filter((w) => w.id !== wallet.id)}
                parents={possibleParents(wallet.id, state.wallets)}
                ordered={ordered}
                tree={tree}
                onCancel={() => setEditing(null)}
                onSubmit={async (name, allocation, picked, carryTo, parent) => {
                  const ok = await run(() =>
                    updateWallet(wallet.id, name, allocation, picked, carryTo, parent),
                  );
                  if (ok) setEditing(null);
                }}
                onRemove={async () => {
                  const ok = await run(() => removeWallet(wallet.id));
                  if (ok) setEditing(null);
                }}
              />
            </li>
          ) : (
            <li
              key={wallet.id}
              className="wallets__item"
              style={{ marginLeft: `${depth * 1.4}rem` }}
            >
              <WalletCard
                wallet={wallet}
                tree={tree}
                money={money}
                carryName={
                  state.wallets.find((w) => w.id === wallet.carry_to)?.name ?? null
                }
                onEdit={() => setEditing(wallet.id)}
              />
            </li>
          ),
        )}
      </ul>

      {/* La somme des enveloppes ne tombe juste que si l'on montre ce qu'elles
          ne couvrent pas : sans cette ligne, la promesse serait fausse. */}
      <dl className="wallets__totals">
        <div>
          <dt>Non alloué</dt>
          <dd>{money(state.unallocated)}</dd>
        </div>
        <div>
          <dt>Solde du compte</dt>
          <dd>{money(state.account_balance)}</dd>
        </div>
      </dl>

      <NewWalletForm
        ordered={ordered}
        tree={tree}
        parents={state.wallets}
        onCreate={run}
      />
    </section>
  );
}

interface CardProps {
  wallet: WalletInfo;
  tree: ReturnType<typeof buildTree>;
  money: (raw: string) => string;
  carryName: string | null;
  onEdit: () => void;
}

function WalletCard({ wallet, tree, money, carryName, onEdit }: CardProps) {
  const available = toCents(wallet.available);
  const balance = toCents(wallet.balance);
  const handed = toCents(wallet.children_allocation);
  // Ce qui a été consommé, en part de ce dont l'enveloppe disposait. Une
  // enveloppe dépassée saturerait la barre : la part est bornée pour que le
  // dessin reste lisible, le chiffre disant seul l'ampleur du dépassement.
  const used = available > 0 ? Math.min(1, 1 - balance / available) : 0;
  const overspent = balance < 0;

  return (
    <div className="wallet">
      <div className="wallet__head">
        <span className="wallet__name">{wallet.name}</span>
        <span className={`wallet__balance${overspent ? " wallet__balance--over" : ""}`}>
          {money(wallet.balance)}
        </span>
        <button type="button" className="wallet__edit" onClick={onEdit}>
          Régler
        </button>
      </div>

      <div className="wallet__bar" aria-hidden="true">
        <div
          className={`wallet__fill${overspent ? " wallet__fill--over" : ""}`}
          style={{ width: `${Math.round(used * 100)}%` }}
        />
      </div>

      <p className="wallet__detail">
        {money(wallet.available)} disponibles — dotation{" "}
        {money(wallet.effective_allocation)}
        {handed !== 0 &&
          ` (${money(wallet.allocation)} dont ${money(wallet.children_allocation)} répartis)`}
        {toCents(wallet.carried_in) !== 0 && `, report ${money(wallet.carried_in)}`}
      </p>

      {/* Une mère débordée par ses filles est un signal à montrer : le corriger
          en silence reviendrait à choisir à sa place quelle enveloppe rogner. */}
      {wallet.overallocated && (
        <p className="wallet__warning">
          Ses enveloppes internes réclament plus que sa dotation.
        </p>
      )}

      {handed !== 0 && (
        <p className="wallet__detail">
          Reste sur toute la branche : {money(wallet.branch_balance)}
        </p>
      )}

      <p className="wallet__categories">
        {wallet.categories.length === 0
          ? "Aucune catégorie rattachée : rien ne l'entame."
          : wallet.categories.map((key) => pathOf(key, tree)).join(" · ")}
      </p>

      {carryName && (
        <p className="wallet__carry">Reliquat de fin de mois versé à {carryName}</p>
      )}
    </div>
  );
}

interface FormProps {
  wallet: WalletInfo;
  others: WalletInfo[];
  /** Enveloppes où celle-ci peut être rangée, sa descendance exclue. */
  parents: WalletInfo[];
  ordered: Array<{ key: string; depth: number }>;
  tree: ReturnType<typeof buildTree>;
  onCancel: () => void;
  onSubmit: (
    name: string,
    allocation: string,
    categories: string[],
    carryTo: number | null,
    parent: number | null,
  ) => Promise<void>;
  onRemove: () => Promise<void>;
}

function WalletForm({
  wallet,
  others,
  parents,
  ordered,
  tree,
  onCancel,
  onSubmit,
  onRemove,
}: FormProps) {
  const [name, setName] = useState(wallet.name);
  const [allocation, setAllocation] = useState(wallet.allocation);
  const [picked, setPicked] = useState<string[]>(wallet.categories);
  const [carryTo, setCarryTo] = useState<string>(
    wallet.carry_to === null ? "" : String(wallet.carry_to),
  );
  const [parent, setParent] = useState<string>(
    wallet.parent === null ? "" : String(wallet.parent),
  );

  const submit = (event: FormEvent) => {
    event.preventDefault();
    void onSubmit(
      name,
      allocation,
      picked,
      carryTo === "" ? null : Number(carryTo),
      parent === "" ? null : Number(parent),
    );
  };

  return (
    <form className="wallet-form" onSubmit={submit}>
      <div className="wallet-form__row">
        <label className="wallet-form__field">
          <span>Nom</span>
          <input value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="wallet-form__field">
          <span>Dotation mensuelle</span>
          <input
            value={allocation}
            inputMode="decimal"
            onChange={(e) => setAllocation(e.target.value)}
          />
        </label>
      </div>

      <label className="wallet-form__field">
        <span>Rangée dans</span>
        <select value={parent} onChange={(e) => setParent(e.target.value)}>
          <option value="">Aucune — enveloppe de premier rang</option>
          {parents.map((candidate) => (
            <option key={candidate.id} value={candidate.id}>
              {candidate.name}
            </option>
          ))}
        </select>
      </label>

      <CategoryChoice ordered={ordered} tree={tree} picked={picked} onChange={setPicked} />

      <label className="wallet-form__field">
        <span>Reliquat de fin de mois</span>
        <select value={carryTo} onChange={(e) => setCarryTo(e.target.value)}>
          <option value="">Reste dans ce portefeuille</option>
          {others.map((other) => (
            <option key={other.id} value={other.id}>
              Versé à {other.name}
            </option>
          ))}
        </select>
      </label>

      <div className="wallet-form__actions">
        <button type="submit">Enregistrer</button>
        <button type="button" onClick={onCancel}>
          Annuler
        </button>
        <button type="button" className="wallet-form__remove" onClick={() => void onRemove()}>
          Supprimer
        </button>
      </div>
    </form>
  );
}

interface ChoiceProps {
  ordered: Array<{ key: string; depth: number }>;
  tree: ReturnType<typeof buildTree>;
  picked: string[];
  onChange: (next: string[]) => void;
}

/**
 * Choix des catégories rattachées.
 *
 * Rattacher une catégorie parente suffit : ses sous-catégories sont comptées
 * avec elle, ce que l'indentation donne à voir sans avoir à l'expliquer.
 */
function CategoryChoice({ ordered, tree, picked, onChange }: ChoiceProps) {
  const toggle = (key: string) =>
    onChange(picked.includes(key) ? picked.filter((k) => k !== key) : [...picked, key]);

  return (
    <fieldset className="wallet-form__categories">
      <legend>Catégories imputées</legend>
      <div className="wallet-form__choices">
        {ordered.map(({ key, depth }) => (
          <label
            key={key}
            className="wallet-form__choice"
            style={{ marginLeft: `${depth * 1}rem` }}
          >
            <input
              type="checkbox"
              checked={picked.includes(key)}
              onChange={() => toggle(key)}
            />
            {tree.labels.get(key) ?? key}
          </label>
        ))}
      </div>
    </fieldset>
  );
}

interface NewProps {
  ordered: Array<{ key: string; depth: number }>;
  tree: ReturnType<typeof buildTree>;
  parents: WalletInfo[];
  onCreate: (action: () => Promise<unknown>) => Promise<boolean>;
}

function NewWalletForm({ ordered, tree, parents, onCreate }: NewProps) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [allocation, setAllocation] = useState("");
  const [picked, setPicked] = useState<string[]>([]);
  const [parent, setParent] = useState("");

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!name.trim()) return;
    const inside = parent === "" ? null : Number(parent);
    if (await onCreate(() => addWallet(name, allocation || "0", picked, inside))) {
      setName("");
      setAllocation("");
      setPicked([]);
      setParent("");
      setOpen(false);
    }
  };

  if (!open) {
    return (
      <button type="button" className="wallets__add" onClick={() => setOpen(true)}>
        Nouveau portefeuille
      </button>
    );
  }

  return (
    <form className="wallet-form" onSubmit={submit}>
      <div className="wallet-form__row">
        <label className="wallet-form__field">
          <span>Nom</span>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Alimentation"
          />
        </label>
        <label className="wallet-form__field">
          <span>Dotation mensuelle</span>
          <input
            value={allocation}
            inputMode="decimal"
            placeholder="300"
            onChange={(e) => setAllocation(e.target.value)}
          />
        </label>
      </div>

      <label className="wallet-form__field">
        <span>Rangée dans</span>
        <select value={parent} onChange={(e) => setParent(e.target.value)}>
          <option value="">Aucune — enveloppe de premier rang</option>
          {parents.map((candidate) => (
            <option key={candidate.id} value={candidate.id}>
              {candidate.name}
            </option>
          ))}
        </select>
      </label>

      <CategoryChoice ordered={ordered} tree={tree} picked={picked} onChange={setPicked} />

      <div className="wallet-form__actions">
        <button type="submit" disabled={!name.trim()}>
          Créer
        </button>
        <button type="button" onClick={() => setOpen(false)}>
          Annuler
        </button>
      </div>

      <p className="wallet-form__hint">
        L'enveloppe est dotée à partir du mois en cours. Ce qui n'est pas dépensé
        rouvre le mois suivant : 300 € dotés, 290 € dépensés, et le mois prochain
        s'ouvre à 310 €. Rangée dans une autre, sa dotation est prise sur celle
        de sa mère.
      </p>
    </form>
  );
}
