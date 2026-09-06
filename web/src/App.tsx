import { useCallback, useEffect, useState } from "react";
import { currentSession, logout, type Session } from "./api/client";
import { LoginScreen } from "./features/auth/LoginScreen";
import { CapitalScreen } from "./features/capital/CapitalScreen";
import "./styles/app.css";

/**
 * Racine de l'application.
 *
 * Tant qu'aucune session n'est ouverte, seul l'écran de connexion s'affiche.
 * Le serveur refuse de toute façon les données sans session : ce test-ci ne
 * fait qu'éviter un aller-retour inutile et une page vide.
 *
 * Les écrans suivants — catégorisation des dépenses, portefeuilles virtuels —
 * viendront s'ajouter comme des dossiers frères sous `features/`, avec ici la
 * navigation entre eux.
 */
export default function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [checked, setChecked] = useState(false);

  useEffect(() => {
    const controller = new AbortController();
    currentSession(controller.signal)
      .then(setSession)
      .catch(() => setSession(null))
      .finally(() => setChecked(true));
    return () => controller.abort();
  }, []);

  const disconnect = useCallback(async () => {
    await logout();
    setSession(null);
  }, []);

  // Éviter l'éclair d'écran de connexion pendant la vérification initiale.
  if (!checked) return null;
  if (!session) return <LoginScreen onAuthenticated={setSession} />;

  return (
    <>
      <CapitalScreen />
      <button type="button" className="signout" onClick={disconnect}>
        {session.email} · Déconnexion
      </button>
    </>
  );
}
