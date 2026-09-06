import { useState, type FormEvent } from "react";
import { login, type Session } from "../../api/client";

interface Props {
  onAuthenticated: (session: Session) => void;
}

/** Écran de connexion, affiché tant qu'aucune session n'est ouverte. */
export function LoginScreen({ onAuthenticated }: Props) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setPending(true);
    setError(null);
    try {
      onAuthenticated(await login(email, password));
    } catch (err) {
      setError(err instanceof Error ? err.message : "connexion refusée");
    } finally {
      setPending(false);
    }
  };

  return (
    <main className="login">
      <form className="login__card" onSubmit={submit}>
        <h1 className="login__title">ecofin</h1>
        <p className="login__intro">Connecte-toi pour accéder à tes comptes.</p>

        <label className="login__field">
          <span>Adresse</span>
          <input
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            autoComplete="username"
            required
            autoFocus
          />
        </label>

        <label className="login__field">
          <span>Mot de passe</span>
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="current-password"
            required
          />
        </label>

        {error && <p className="login__error">{error}</p>}

        <button type="submit" className="login__submit" disabled={pending}>
          {pending ? "Connexion…" : "Se connecter"}
        </button>

        <p className="login__hint">
          Pas encore de compte ? Crée-le depuis le CLI :{" "}
          <code>ecofin user add &lt;adresse&gt;</code>
        </p>
      </form>
    </main>
  );
}
