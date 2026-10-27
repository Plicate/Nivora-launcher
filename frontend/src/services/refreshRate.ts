// requestAnimationFrame не показывает частоту экрана: браузер ограничивает кадры, а компоновщик может объединять их
// ноль означает неизвестную частоту
export function measuredRefreshHz(): number {
  return 0
}

export function measureRefreshHz(): Promise<number> {
  return Promise.resolve(0)
}
