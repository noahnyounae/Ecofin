// Placement des points sur l'axe du temps, et choix des graduations.
//
// Extrait du composant pour être éprouvable : c'est ici que s'est logé un
// défaut visible seulement à l'écran — des abscisses qui reculaient au sein
// d'une journée, faisant repartir la courbe en arrière.

/**
 * Créneaux par journée sur l'axe du temps.
 *
 * Les opérations n'ont qu'une date, sans heure : sans décalage, celles d'un
 * même jour se superposent exactement.
 */
export const SLOTS_PER_DAY = 8;

/** Nombre maximal de graduations sur l'axe des dates. */
export const MAX_TICKS = 8;

export const DAY_MS = 86_400_000;

/** Ce dont le placement a besoin : une date et rien d'autre. */
interface Dated {
  date: string;
}

/** Un point placé sur l'axe. */
export interface Placed<T> {
  point: T;
  /** Abscisse effective : le jour, décalé de son créneau. */
  timestamp: number;
  /** Minuit de la journée, pour les graduations. */
  dayStart: number;
}

/**
 * Répartit les opérations d'une même journée sur des créneaux successifs.
 *
 * Le diviseur vaut [`SLOTS_PER_DAY`] au minimum, et le nombre d'opérations
 * au-delà. Deux propriétés en découlent, et elles comptent toutes les deux :
 *
 * - **les abscisses croissent** avec l'ordre des points. Réutiliser les
 *   créneaux en boucle ferait repartir la courbe en arrière à l'intérieur de
 *   la journée, et les traits se superposeraient ;
 * - **aucune abscisse n'est partagée**, donc aucun point n'en masque un autre.
 *
 * Le décalage reste enfermé dans la journée : le dernier point d'un jour tombe
 * avant minuit du suivant, et l'axe du temps garde son sens.
 */
export function placePoints<T extends Dated>(points: T[]): Array<Placed<T>> {
  // Le pas de division dépend du nombre d'opérations du jour : il faut donc
  // les compter avant d'en placer une seule.
  const perDay = new Map<string, number>();
  for (const point of points) {
    perDay.set(point.date, (perDay.get(point.date) ?? 0) + 1);
  }

  const rank = new Map<string, number>();

  return points.map((point) => {
    const seen = rank.get(point.date) ?? 0;
    rank.set(point.date, seen + 1);

    const divisions = Math.max(perDay.get(point.date) ?? 1, SLOTS_PER_DAY);
    const dayStart = new Date(point.date).getTime();

    return {
      point,
      timestamp: dayStart + (seen * DAY_MS) / divisions,
      dayStart,
    };
  });
}

/**
 * Choisit les graduations parmi les journées réellement présentes.
 *
 * Laisser la bibliothèque les placer à intervalles réguliers les ferait tomber
 * sur des dates sans opération, ce qui suggère une continuité que la courbe
 * n'a pas : chaque point est une opération, pas une mesure périodique.
 *
 * La répartition se fait dans le *temps* et non un point sur N. Réparties par
 * rang, les graduations s'agglutineraient là où les opérations sont denses.
 */
export function selectTicks(days: number[], max: number): number[] {
  const unique = [...new Set(days)].sort((a, b) => a - b);
  if (unique.length <= max) return unique;

  const first = unique[0]!;
  const last = unique[unique.length - 1]!;
  const span = last - first;
  const chosen = new Set<number>();

  for (let i = 0; i < max; i += 1) {
    const target = first + (span * i) / (max - 1);
    let nearest = unique[0]!;
    for (const value of unique) {
      if (Math.abs(value - target) < Math.abs(nearest - target)) nearest = value;
    }
    chosen.add(nearest);
  }
  return [...chosen].sort((a, b) => a - b);
}

const YEAR_MS = 365 * DAY_MS;

/**
 * Format de date adapté à l'étendue affichée.
 *
 * Sur plusieurs années, « 29 août » ne dit pas de quelle année il s'agit ; sur
 * une semaine, l'année est du bruit.
 */
export function tickFormatter(span: number): (value: number) => string {
  const options: Intl.DateTimeFormatOptions =
    span > 2 * YEAR_MS
      ? { month: "short", year: "2-digit" }
      : { day: "2-digit", month: "short" };
  const format = new Intl.DateTimeFormat("fr-FR", options);
  return (value) => format.format(new Date(value));
}
