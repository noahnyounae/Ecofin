import type { Period } from "../../api/client";
import { shiftPeriod, type Direction } from "./period";

/** Raccourcis courants, exprimés en jours écoulés depuis maintenant. */
const PRESETS: Array<{ label: string; days: number | null }> = [
  { label: "3 j", days: 3 },
  { label: "7 j", days: 7 },
  { label: "30 j", days: 30 },
  { label: "3 mois", days: 90 },
  { label: "1 an", days: 365 },
  { label: "Tout", days: null },
];

interface Props {
  period: Period;
  onChange: (period: Period) => void;
  /** Bornes de l'historique, pour empêcher de viser en dehors. */
  bounds: { first: string | null; last: string | null };
}

/**
 * Sélection de la période, avec date **et heure**.
 *
 * `datetime-local` attend `AAAA-MM-JJTHH:MM` sans fuseau, alors que l'API
 * raisonne en RFC 3339 UTC : la conversion se fait dans les deux sens ici, pour
 * que le reste du front n'ait à manipuler que de l'UTC.
 *
 * Les deux flèches déplacent la fenêtre de sa propre largeur, ce qui permet de
 * parcourir l'historique par tranches comparables. Elles ne touchent qu'aux
 * bornes : la recherche en cours reste appliquée, et c'est bien l'intérêt —
 * suivre un même commerçant d'une période à la suivante.
 */
export function PeriodPicker({ period, onChange, bounds }: Props) {
  const applyPreset = (days: number | null) => {
    if (days === null) {
      onChange({ from: null, to: null });
      return;
    }
    const to = new Date();
    const from = new Date(to.getTime() - days * 86_400_000);
    onChange({ from: from.toISOString(), to: to.toISOString() });
  };

  const move = (direction: Direction) => {
    const moved = shiftPeriod(period, direction);
    if (moved) onChange(moved);
  };

  // Une période non bornée n'a pas de largeur à reporter : la flèche est
  // désactivée plutôt que laissée sans effet sous le doigt.
  const shiftable = shiftPeriod(period, "next") !== null;

  return (
    <div className="period">
      <div className="period__fields">
        <button
          type="button"
          className="period__step"
          onClick={() => move("previous")}
          disabled={!shiftable}
          aria-label="Période précédente, de même durée"
          title={
            shiftable
              ? "Reculer d'une période entière"
              : "Choisis une période bornée des deux côtés"
          }
        >
          ‹
        </button>
        <label className="period__field">
          <span className="period__label">Du</span>
          <input
            type="datetime-local"
            value={toLocalInput(period.from)}
            min={toLocalInput(bounds.first)}
            max={toLocalInput(period.to ?? bounds.last)}
            onChange={(event) =>
              onChange({ ...period, from: fromLocalInput(event.target.value) })
            }
          />
        </label>
        <label className="period__field">
          <span className="period__label">Au</span>
          <input
            type="datetime-local"
            value={toLocalInput(period.to)}
            min={toLocalInput(period.from ?? bounds.first)}
            max={toLocalInput(bounds.last)}
            onChange={(event) =>
              onChange({ ...period, to: fromLocalInput(event.target.value) })
            }
          />
        </label>
        <button
          type="button"
          className="period__step"
          onClick={() => move("next")}
          disabled={!shiftable}
          aria-label="Période suivante, de même durée"
          title={
            shiftable
              ? "Avancer d'une période entière"
              : "Choisis une période bornée des deux côtés"
          }
        >
          ›
        </button>
      </div>

      <div className="period__presets">
        {PRESETS.map((preset) => (
          <button
            key={preset.label}
            type="button"
            className="period__preset"
            onClick={() => applyPreset(preset.days)}
          >
            {preset.label}
          </button>
        ))}
      </div>
    </div>
  );
}

/** UTC → valeur locale attendue par `datetime-local`. */
function toLocalInput(iso: string | null): string {
  if (!iso) return "";
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return "";
  // `toISOString` repasse en UTC : on retranche le décalage pour que l'heure
  // affichée soit bien celle du fuseau de l'utilisateur.
  const local = new Date(parsed.getTime() - parsed.getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 16);
}

/** Valeur locale de `datetime-local` → UTC pour l'API. */
function fromLocalInput(value: string): string | null {
  if (!value) return null;
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? null : parsed.toISOString();
}
