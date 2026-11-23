function wheelScrollTarget(event: WheelEvent) {
  for (const element of event.composedPath()) {
    if (!(element instanceof HTMLElement) || element.scrollHeight <= element.clientHeight) continue;
    if (['auto', 'scroll'].includes(getComputedStyle(element).overflowY)) return element;
  }
  return null;
}

export function installSmoothWheelScroll() {
  const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)');
  let active: HTMLElement | null = null;
  let targetTop = 0;
  let position = 0;
  let lastTop = 0;
  let lastFrameTime = 0;
  let frame = 0;

  function stop() {
    cancelAnimationFrame(frame);
    frame = 0;
    active = null;
  }

  function animate(now: number) {
    frame = 0;
    if (!active?.isConnected || active.scrollTop !== lastTop) {
      stop();
      return;
    }
    targetTop = Math.max(0, Math.min(active.scrollHeight - active.clientHeight, targetTop));
    const remaining = targetTop - position;
    const progress = 1 - Math.exp(-(now - lastFrameTime) / 65);
    // WebView 会取整滚动位置，保留浮点进度以免减速尾段停滞。
    position = Math.abs(remaining) <= .5 ? targetTop : position + remaining * progress;
    active.scrollTop = position;
    lastTop = active.scrollTop;
    lastFrameTime = now;
    if (Math.abs(remaining) > .5) frame = requestAnimationFrame(animate);
  }

  function onWheel(event: WheelEvent) {
    if (event.defaultPrevented || !event.cancelable || event.ctrlKey || event.metaKey || event.shiftKey || !event.deltaY || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
    if (reducedMotion.matches) return;
    const element = wheelScrollTarget(event);
    if (!element) return;

    const top = element.scrollTop;
    const max = element.scrollHeight - element.clientHeight;
    if (event.deltaY < 0 && top <= 0 || event.deltaY > 0 && top >= max) {
      stop();
      return;
    }
    const style = getComputedStyle(element);
    const unit = event.deltaMode === WheelEvent.DOM_DELTA_LINE
      ? parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.2
      : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? element.clientHeight : 1;
    const delta = event.deltaY * unit;
    if (active !== element || top !== lastTop) {
      stop();
      active = element;
      targetTop = top;
      position = top;
    } else if (delta * (targetTop - top) < 0) {
      targetTop = top;
    }

    event.preventDefault();
    targetTop = Math.max(0, Math.min(max, targetTop + delta));
    lastTop = top;
    if (!frame) {
      lastFrameTime = performance.now();
      frame = requestAnimationFrame(animate);
    }
  }

  document.addEventListener('wheel', onWheel, { passive: false, capture: true });
  document.addEventListener('pointerdown', stop, true);
  document.addEventListener('keydown', stop, true);
  reducedMotion.addEventListener('change', stop);
  return () => {
    stop();
    document.removeEventListener('wheel', onWheel, true);
    document.removeEventListener('pointerdown', stop, true);
    document.removeEventListener('keydown', stop, true);
    reducedMotion.removeEventListener('change', stop);
  };
}
