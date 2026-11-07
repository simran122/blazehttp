// The differential correctness gate: every parser must agree with the bytes it was given and with our parser on
// every head of every workload before anything is timed.
import { differences } from './protocol.ts';
import type { CheckReport, FeedReport, FieldView } from './protocol.ts';

export interface GateParser extends FieldView {
  readonly id: string;
}

export interface GateWorkload {
  readonly name: string;
  readonly heads: readonly Buffer[];
}

/** `parser id → workload name → report`. */
export type CheckReports = ReadonlyMap<string, ReadonlyMap<string, CheckReport>>;

/**
 * Every gate failure (empty when the gate passes). Ground truth first: each parser must report one head per input
 * head, each as long as the input head (every input is exactly one complete head). Then every parser's facts and
 * fields must match the reference parser's, as its field view allows.
 */
export function gateFailures(
  parsers: readonly GateParser[],
  workloads: readonly GateWorkload[],
  reports: CheckReports,
  referenceId: string,
): string[] {
  return parsers.flatMap((parser) =>
    workloads.flatMap((w) => {
      const at = `${parser.id} ${w.name}`;
      const report = reports.get(parser.id)?.get(w.name);
      if (report === undefined) return [`${at}: no report`];
      if (report.heads.length !== w.heads.length) {
        return [`${at}: ${String(report.heads.length)} heads, expected ${String(w.heads.length)}`];
      }
      const truth = report.heads.flatMap((head, i) => {
        const expected = w.heads[i]?.length;
        return head.headLen === expected
          ? []
          : [`${at}: head ${String(i)} length ${String(head.headLen)}, expected ${String(expected)}`];
      });
      const reference = reports.get(referenceId)?.get(w.name);
      if (reference === undefined) return [...truth, `${at}: no reference report`];
      return [...truth, ...differences(reference.heads, report.heads, parser).map((d) => `${at}: ${d}`)];
    }),
  );
}

/** A feed must end with the whole input consumed as one head. */
export function feedFailure(parserId: string, workload: GateWorkload, report: FeedReport): string | undefined {
  const expected = workload.heads[0]?.length;
  return report.headLen === expected
    ? undefined
    : `${parserId} ${workload.name}: fed head length ${String(report.headLen)}, expected ${String(expected)}`;
}
