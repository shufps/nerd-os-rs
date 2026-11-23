export interface IAlertSettings {
  webhookUrl: string;
  watchdogEnable: boolean;
  blockFoundEnable: boolean;
  bestDiffEnable: boolean;
  coinbaseVerifyEnable: boolean;
  heartbeatHours?: number;
  showBlockFoundScreen: boolean;
}
