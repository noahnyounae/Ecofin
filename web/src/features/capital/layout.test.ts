import { describe, expect, it } from "vitest";
import { DAY_MS, MAX_TICKS, SLOTS_PER_DAY, placePoints, selectTicks, tickFormatter } from "./layout";

/** Opérations d'une même journée, dans l'ordre où elles arrivent. */
function day(date: string, count: number) {
  return Array.from({ length: count }, (_, i) => ({ date, id: `${date}-${i}` }));
}

describe("placePoints", () => {
  it("laisse une opération seule au début de sa journée", () => {
    const [placed] = placePoints(day("2026-08-30", 1));
    expect(placed!.timestamp).toBe(placed!.dayStart);
  });

  it("répartit une journée sur des créneaux successifs", () => {
    const placed = placePoints(day("2026-08-30", 4));
    const heures = placed.map((p) => (p.timestamp - p.dayStart) / 3_600_000);
    expect(heures).toEqual([0, 3, 6, 9]);
  });

  it("subdivise davantage une journée chargée", () => {
    const placed = placePoints(day("2026-08-30", 12));
    const heures = placed.map((p) => (p.timestamp - p.dayStart) / 3_600_000);
    expect(heures).toEqual([0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22]);
  });

  // Le défaut qui a motivé cette extraction : les créneaux se réutilisaient en
  // boucle, l'abscisse repartait en arrière et les traits se superposaient au
  // sein de la journée.
  it("ne fait jamais reculer l'abscisse", () => {
    const points = [...day("2026-08-28", 12), ...day("2026-08-29", 3), ...day("2026-08-30", 9)];
    const timestamps = placePoints(points).map((p) => p.timestamp);

    for (let i = 1; i < timestamps.length; i += 1) {
      expect(timestamps[i]!).toBeGreaterThan(timestamps[i - 1]!);
    }
  });

  it("ne place jamais deux points au même endroit", () => {
    const points = [...day("2026-08-28", 12), ...day("2026-08-29", 1), ...day("2026-08-30", 5)];
    const timestamps = placePoints(points).map((p) => p.timestamp);
    expect(new Set(timestamps).size).toBe(timestamps.length);
  });

  // Sans quoi une opération d'un jour se retrouverait dessinée le lendemain.
  it("garde chaque point à l'intérieur de sa journée", () => {
    for (const count of [1, 2, 8, 12, 40]) {
      for (const placed of placePoints(day("2026-08-30", count))) {
        expect(placed.timestamp).toBeGreaterThanOrEqual(placed.dayStart);
        expect(placed.timestamp).toBeLessThan(placed.dayStart + DAY_MS);
      }
    }
  });

  it("n'utilise jamais moins de créneaux que le minimum fixé", () => {
    const placed = placePoints(day("2026-08-30", 2));
    const ecart = (placed[1]!.timestamp - placed[0]!.timestamp) / DAY_MS;
    expect(ecart).toBeCloseTo(1 / SLOTS_PER_DAY);
  });

  it("accepte une liste vide", () => {
    expect(placePoints([])).toEqual([]);
  });
});

describe("selectTicks", () => {
  const jour = (iso: string) => new Date(iso).getTime();

  it("retient toutes les journées quand elles sont peu nombreuses", () => {
    const jours = [jour("2026-08-28"), jour("2026-08-29"), jour("2026-08-30")];
    expect(selectTicks(jours, MAX_TICKS)).toEqual(jours);
  });

  it("dédoublonne les journées répétées", () => {
    const j = jour("2026-08-30");
    expect(selectTicks([j, j, j], MAX_TICKS)).toEqual([j]);
  });

  // Une graduation posée sur une date sans opération suggérerait une
  // continuité que la courbe n'a pas.
  it("ne retient que des journées réellement présentes", () => {
    const jours = Array.from({ length: 400 }, (_, i) => jour("2024-01-01") + i * DAY_MS);
    for (const tick of selectTicks(jours, MAX_TICKS)) {
      expect(jours).toContain(tick);
    }
  });

  it("respecte le nombre maximal demandé", () => {
    const jours = Array.from({ length: 400 }, (_, i) => jour("2024-01-01") + i * DAY_MS);
    expect(selectTicks(jours, MAX_TICKS).length).toBeLessThanOrEqual(MAX_TICKS);
  });

  it("conserve les bornes de la période", () => {
    const jours = Array.from({ length: 400 }, (_, i) => jour("2024-01-01") + i * DAY_MS);
    const ticks = selectTicks(jours, MAX_TICKS);
    expect(ticks[0]).toBe(jours[0]);
    expect(ticks[ticks.length - 1]).toBe(jours[jours.length - 1]);
  });

  // Sur une répartition déséquilibrée, moins de graduations sont possibles :
  // aucune ne peut se poser là où il ne s'est rien passé.
  it("rend moins de graduations quand les journées sont mal réparties", () => {
    const jours = [jour("2022-01-01"), ...Array.from({ length: 200 }, (_, i) => jour("2026-08-01") + i * DAY_MS)];
    expect(selectTicks(jours, MAX_TICKS).length).toBeLessThan(MAX_TICKS);
  });
});

describe("tickFormatter", () => {
  it("montre l'année sur une longue période", () => {
    const rendu = tickFormatter(4 * 365 * DAY_MS)(new Date("2026-08-30").getTime());
    expect(rendu).toMatch(/26/);
  });

  it("montre le jour sur une courte période", () => {
    const rendu = tickFormatter(30 * DAY_MS)(new Date("2026-08-30").getTime());
    expect(rendu).toMatch(/30/);
  });
});
