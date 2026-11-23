import { IUpdateStatus } from 'src/app/models/IUpdateStatus';

/**
 * What a polled `GET /api/system/OTA/github` status means for the UI while
 * an update started from GitHub is in progress.
 *
 * Device side (ota_factory*.rs): `pending` until the update task picks the
 * request up, `running` while it works through
 * resolving_url -> updating_fw -> downloading_www -> updating_www ->
 * [erasing_nvs] -> rebooting. On a failure the step becomes `error` and the
 * task ends (`running` false); a failure after the board was shut down
 * restarts the device, which then reports `idle` and not running.
 *
 * - `running`:   keep polling
 * - `rebooting`: success, the device restarts into the new firmware
 * - `failed`:    terminal error: stop the spinner and show the error
 */
export type GithubOtaOutcome = 'running' | 'rebooting' | 'failed';

export function githubOtaOutcome(status: IUpdateStatus | null | undefined): GithubOtaOutcome {
  if (!status) return 'running';
  if (status.step === 'rebooting') return 'rebooting';
  if (status.step === 'error') return 'failed';
  // the task is done without reaching `rebooting` (e.g. the device restarted
  // into the old firmware after a failed flash write)
  if (!status.pending && !status.running) return 'failed';
  return 'running';
}

/** Steps that say where an update failed (not idle/error/unknown). */
export function isProgressStep(step: string | null | undefined): boolean {
  return !!step && step !== 'idle' && step !== 'error';
}
