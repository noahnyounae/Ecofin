import type { Period } from "../../api/client";

/** Sens du déplacement : vers le passé, ou vers l'avenir. */
export type Direction = "previous" | "next";

/**
 * Déplace la période d'exactement sa propre durée.
 *
 * Une fenêtre d'un an couvrant 2022–2023 devient 2023–2024 vers l'avenir, et
 * 2021–2022 vers le passé. La largeur est conservée, et deux fenêtres
 * successives se touchent sans se recouvrir ni laisser de trou : c'est ce qui
 * permet de parcourir l'historique par tranches comparables.
 *
 * Le pas est la durée mesurée entre les deux bornes, et non un mois ou une
 * année de calendrier : « 30 j » avance de trente jours, quels que soient les
 * mois traversés.
 *
 * Rend `null` quand le déplacement n'a pas de sens — période non bornée des
 * deux côtés, donc sans largeur à reporter, ou bornes illisibles. L'appelant
 * s'en sert pour désactiver la commande plutôt que de la laisser sans effet.
 */
export function shiftPeriod(period: Period, direction: Direction): Period | null {
  if (!period.from || !period.to) return null;

  const from = new Date(period.from).getTime();
  const to = new Date(period.to).getTime();
  if (Number.isNaN(from) || Number.isNaN(to)) return null;

  // Une largeur nulle ou inversée ne donnerait aucun déplacement visible.
  const width = to - from;
  if (width <= 0) return null;

  const step = direction === "next" ? width : -width;
  return {
    from: new Date(from + step).toISOString(),
    to: new Date(to + step).toISOString(),
  };
}
