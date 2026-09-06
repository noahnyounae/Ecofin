import { describe, expect, it } from "vitest";
import { shiftPeriod } from "./period";

const AN_2022 = { from: "2022-01-01T00:00:00.000Z", to: "2023-01-01T00:00:00.000Z" };

describe("shiftPeriod", () => {
  // L'exemple qui a motivé la commande : une fenêtre d'un an se déplace d'un
  // an, et les fenêtres successives se touchent bord à bord.
  it("avance d'un an une fenêtre d'un an", () => {
    expect(shiftPeriod(AN_2022, "next")).toEqual({
      from: "2023-01-01T00:00:00.000Z",
      to: "2024-01-01T00:00:00.000Z",
    });
  });

  it("recule d'un an une fenêtre d'un an", () => {
    expect(shiftPeriod(AN_2022, "previous")).toEqual({
      from: "2021-01-01T00:00:00.000Z",
      to: "2022-01-01T00:00:00.000Z",
    });
  });

  // Sans cette propriété, parcourir l'historique laisserait des trous ou
  // compterait deux fois les mêmes opérations.
  it("fait se toucher les fenêtres sans recouvrement ni trou", () => {
    const suivante = shiftPeriod(AN_2022, "next")!;
    expect(suivante.from).toBe(AN_2022.to);

    const precedente = shiftPeriod(AN_2022, "previous")!;
    expect(precedente.to).toBe(AN_2022.from);
  });

  it("conserve la largeur, quelle qu'elle soit", () => {
    const largeur = (p: { from: string | null; to: string | null }) =>
      new Date(p.to!).getTime() - new Date(p.from!).getTime();
    const semaine = { from: "2026-08-03T09:30:00.000Z", to: "2026-08-10T09:30:00.000Z" };

    expect(largeur(shiftPeriod(semaine, "next")!)).toBe(largeur(semaine));
    expect(largeur(shiftPeriod(semaine, "previous")!)).toBe(largeur(semaine));
  });

  it("revient au point de départ en faisant l'aller puis le retour", () => {
    const aller = shiftPeriod(AN_2022, "next")!;
    expect(shiftPeriod(aller, "previous")).toEqual(AN_2022);
  });

  // Le pas est une durée, pas un mois de calendrier : trente jours restent
  // trente jours, même en traversant février.
  it("compte en durée écoulée, non en mois de calendrier", () => {
    const janvier = { from: "2026-01-01T00:00:00.000Z", to: "2026-01-31T00:00:00.000Z" };
    expect(shiftPeriod(janvier, "next")).toEqual({
      from: "2026-01-31T00:00:00.000Z",
      to: "2026-03-02T00:00:00.000Z",
    });
  });

  it("préserve l'heure des bornes", () => {
    const apresMidi = { from: "2026-08-01T14:45:00.000Z", to: "2026-08-08T14:45:00.000Z" };
    expect(shiftPeriod(apresMidi, "next")!.from).toBe("2026-08-08T14:45:00.000Z");
  });

  // « Tout » n'a pas de largeur à reporter : la commande doit être désactivée
  // plutôt que rester sans effet sous le doigt.
  it("refuse une période non bornée", () => {
    expect(shiftPeriod({ from: null, to: null }, "next")).toBeNull();
    expect(shiftPeriod({ from: AN_2022.from, to: null }, "next")).toBeNull();
    expect(shiftPeriod({ from: null, to: AN_2022.to }, "previous")).toBeNull();
  });

  it("refuse une période de durée nulle ou inversée", () => {
    expect(shiftPeriod({ from: AN_2022.from, to: AN_2022.from }, "next")).toBeNull();
    expect(shiftPeriod({ from: AN_2022.to, to: AN_2022.from }, "next")).toBeNull();
  });

  it("refuse une borne illisible plutôt que de produire une date invalide", () => {
    expect(shiftPeriod({ from: "charabia", to: AN_2022.to }, "next")).toBeNull();
  });
});
