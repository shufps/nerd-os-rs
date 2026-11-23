export interface IAlertSettings {
  webhookUrl: string;
  watchdogEnable: boolean;
  blockFoundEnable: boolean;
  bestDiffEnable: boolean;
  coinbaseVerifyEnable: boolean;
  heartbeatEnable?: boolean;
  showBlockFoundScreen: boolean;
}
