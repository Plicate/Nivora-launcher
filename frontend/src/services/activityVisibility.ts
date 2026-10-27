export function shouldOpenActivityAfterSessionStart(showPanel: boolean, dismissedForRun: boolean): boolean {
  return showPanel && !dismissedForRun
}
