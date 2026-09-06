import { useMemo } from "react";
import {
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import type { CapitalPoint } from "../../api/types";
import { date, money, moneyCompact } from "../../format";
import { MAX_TICKS, placePoints, selectTicks, tickFormatter } from "./layout";
import { pathOf, type CategoryTree } from "../categories/tree";

/**
 * Au-delà de ce nombre de points, les pastilles ne sont plus dessinées.
 *
 * Sur quatre ans, 1782 pastilles forment une bande illisible et alourdissent le
 * rendu. La courbe reste cliquable : c'est la pastille active, au survol, qui
 * prend le relais.
 */
const DOT_LIMIT = 150;



interface Props {
  points: CapitalPoint[];
  currency: string;
  /** Journée mise en évidence, s'il y en a une. */
  selectedDay: string | null;
  /** Nom stable → libellé français des secteurs. */
  /** L'arbre des secteurs : la bulle affiche le chemin complet. */
  tree: CategoryTree;
  onSelect: (point: CapitalPoint) => void;
}

/** Un point prêt pour Recharts : placé sur l'axe, et son solde en nombre. */
interface Datum {
  point: CapitalPoint;
  /** Recharts a besoin d'un nombre : la précision décimale reste dans `point`. */
  balance: number;
  timestamp: number;
  dayStart: number;
}

export function CapitalChart({
  points,
  currency,
  selectedDay,
  tree,
  onSelect,
}: Props) {
  const data = useMemo<Datum[]>(
    () =>
      placePoints(points).map((placed) => ({
        ...placed,
        balance: Number(placed.point.balance),
      })),
    [points],
  );

  // Les graduations se posent sur les journées, pas sur les créneaux : une
  // date affichée à six heures du matin n'aurait aucun sens.
  const ticks = useMemo(
    () => selectTicks(data.map((d) => d.dayStart), MAX_TICKS),
    [data],
  );

  const formatTick = useMemo(() => {
    const first = data[0]?.timestamp ?? 0;
    const last = data[data.length - 1]?.timestamp ?? 0;
    return tickFormatter(last - first);
  }, [data]);

  if (data.length === 0) {
    return (
      <div className="chart chart--empty">
        <p>Aucune opération sur cette période.</p>
      </div>
    );
  }

  const showDots = data.length <= DOT_LIMIT;

  return (
    <div className="chart">
      <ResponsiveContainer width="100%" height="100%">
        <LineChart
          data={data}
          margin={{ top: 16, right: 20, bottom: 8, left: 8 }}
          onClick={(event) => {
            // Recharts remonte le point survolé au moment du clic.
            const datum = event?.activePayload?.[0]?.payload as Datum | undefined;
            if (datum) onSelect(datum.point);
          }}
        >
          <CartesianGrid stroke="var(--line)" strokeDasharray="2 4" vertical={false} />
          <XAxis
            dataKey="timestamp"
            type="number"
            scale="time"
            domain={["dataMin", "dataMax"]}
            // Graduations imposées : chacune tombe sur une opération existante.
            ticks={ticks}
            tickFormatter={formatTick}
            stroke="var(--muted)"
            tickLine={true}
          />
          <YAxis
            dataKey="balance"
            tickFormatter={moneyCompact}
            stroke="var(--muted)"
            tickLine={false}
            axisLine={false}
            width={72}
          />
          <Tooltip
            cursor={{ stroke: "var(--muted)", strokeDasharray: "3 3" }}
            content={({ active, payload }) => {
              if (!active || !payload?.length) return null;
              const { point } = payload[0]!.payload as Datum;
              return (
                <div className="tooltip">
                  <div className="tooltip__date">{date(point.date)}</div>
                  <div className="tooltip__balance">{money(point.balance, currency)}</div>
                  <div className="tooltip__label">{point.description}</div>
                  {tree.labels.has(point.category) && (
                    <div className="tooltip__category">
                      {pathOf(point.category, tree)}
                    </div>
                  )}
                  <div
                    className={`tooltip__amount${
                      Number(point.amount) < 0 ? " tooltip__amount--debit" : ""
                    }`}
                  >
                    {money(point.amount, currency)}
                  </div>
                </div>
              );
            }}
          />
          <Line
            type="monotone"
            dataKey="balance"
            stroke="var(--accent)"
            strokeWidth={2}
            isAnimationActive={false}
            dot={
              showDots
                ? (props) => {
                    const datum = props.payload as Datum;
                    // Toute la journée s'illumine, puisque c'est elle qu'on
                    // sélectionne.
                    const selected = datum.point.date === selectedDay;
                    return (
                      <circle
                        key={datum.point.transaction_id}
                        cx={props.cx}
                        cy={props.cy}
                        r={selected ? 6 : 3}
                        className={`dot${selected ? " dot--selected" : ""}`}
                      />
                    );
                  }
                : false
            }
            activeDot={{ r: 6, className: "dot dot--active" }}
          />
        </LineChart>
      </ResponsiveContainer>

      {!showDots && (
        <p className="chart__hint">
          Les pastilles apparaissent en dessous de {DOT_LIMIT} opérations. Survole
          ou clique la courbe pour ouvrir le détail.
        </p>
      )}
    </div>
  );
}
