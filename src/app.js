let animator;
let bubble;
let lastBubble = '';
let toolLockUntil = 0;
let idleStart = 0;
let idleAnimTimer = null;
let permissionPending = false;
let permissionTimer = null;

// Phase 1 new vars
let lastCoreSignal = '';
let lastOverlay = '';
let inputPending = false;
let lastEventTime = 0;
let fallbackTimer = null;

// Phase 3 new vars
let clickStart = null;

async function init() {
  const resp = await fetch('validation.json');
  const validationData = await resp.json();
  const spriteEl = document.getElementById('sprite');
  const bubbleEl = document.getElementById('bubble');
  animator = new SpriteAnimator(spriteEl, validationData);
  bubble = new Bubble(bubbleEl);
  animator.play('waving');
  bubble.show('爱弥斯已上线~');
  window._petBubble = bubble;
  window._petAnimator = animator;

  const ipc = window.__TAURI_INTERNALS__;
  document.addEventListener('mousedown', (e) => {
    if (e.button !== 0) return;
    try { if (ipc && ipc.invoke) ipc.invoke('start_drag'); } catch (_) {}
  });

  // Phase 3: click sprite → quick menu (distinguish click from drag)
  spriteEl.addEventListener('mousedown', (e) => {
    if (e.button !== 0) return;
    clickStart = { x: e.clientX, y: e.clientY, time: Date.now() };
  });
  spriteEl.addEventListener('mouseup', (e) => {
    if (!clickStart) return;
    const dx = Math.abs(e.clientX - clickStart.x);
    const dy = Math.abs(e.clientY - clickStart.y);
    const dt = Date.now() - clickStart.time;
    clickStart = null;
    if (dx < 5 && dy < 5 && dt < 400) {
      bubble.showQuickMenu();
    }
  });

  setupEventListener();
  setupInteractiveInput();
  setupQuickMenu();
  setupConfirmButtons();
  startFallbackPoll();

  // Expose for bubble.js choice buttons
  window._sendUserInput = sendUserInput;
}

// ========== Tauri event listener (primary channel) ==========

function setupEventListener() {
  window.__TAURI_INTERNALS__.listen('state-change', (event) => {
    lastEventTime = Date.now();
    const { animation, bubble: bubbleText, core_signal, tool_label, overlay,
            input_type, options } = event.payload;

    if (animation) {
      animator.play(animation);
    }

    updateBubbleStates(bubbleText, core_signal, overlay, input_type, options);
    handleIdleAnimation(animation);

    lastCoreSignal = core_signal;
    lastOverlay = overlay || '';
  });
}

// ========== Fallback polling (2s, only when Tauri events stale) ==========

function startFallbackPoll() {
  if (fallbackTimer) return;
  fallbackTimer = setInterval(async () => {
    // Only activate fallback if no Tauri event for 3s
    if (Date.now() - lastEventTime < 3000) return;
    try {
      const r = await fetch('http://127.0.0.1:9527/api/current');
      if (r.ok) {
        const data = await r.json();
        if (data.animation) {
          animator.play(data.animation);
        }
        if (data.core_signal) {
          updateBubbleStates(data.bubble, data.core_signal, data.overlay);
          handleIdleAnimation(data.animation);
        }
      }
    } catch (_) {}
    // Phase 3: also poll pending state to recover input UI
    try {
      const pr = await fetch('http://127.0.0.1:9527/api/user/pending');
      if (pr.ok) {
        const pd = await pr.json();
        if (pd.waiting && !inputPending) {
          inputPending = true;
          const it = pd.input_type || 'text';
          const opts = (pd.input_type === 'select') ? pd.options : null;
          bubble.showInteractive('等待输入...', it, opts, '输入...');
          bubble.startPendingPoll();
        }
      }
    } catch (_) {}
  }, 2000);
}

// ========== Bubble state machine (core_signal-driven) ==========

function updateBubbleStates(bubbleText, coreSignal, overlay, inputType, options) {
  // --- input overlay: show interactive bubble ---
  if (overlay === 'input') {
    if (!inputPending) {
      inputPending = true;
      const it = inputType || 'text';
      const opts = (inputType === 'select') ? options : null;
      bubble.showInteractive(bubbleText || '请输入...', it, opts, '输入...');
      bubble.startPendingPoll();
    }
    return;
  }

  // Hide interactive bubble when overlay clears
  if (inputPending && overlay !== 'input') {
    inputPending = false;
    bubble.hideInteractive();
  }

  // --- permission overlay ---
  if (overlay === 'permission') {
    if (!permissionPending) {
      permissionPending = true;
      permissionTimer = setInterval(() => {
        if (permissionPending && bubble) {
          bubble.hide();
          bubble.show('等待指示...');
        }
      }, 300);
      setTimeout(() => {
        if (permissionPending) { exitPermission(); bubble.hide(); }
      }, 120000);
    }
    if (bubbleText && bubbleText !== lastBubble) {
      lastBubble = bubbleText;
      bubble.showPersistent(bubbleText);
    }
    return;
  }

  // Clear permission when transitioning away
  if (permissionPending && overlay !== 'permission') {
    exitPermission();
  }

  const now = Date.now();

  // --- running: tool bubble, persistent ---
  if (coreSignal === 'running') {
    if (bubbleText && bubbleText !== lastBubble) {
      lastBubble = bubbleText;
      bubble.showPersistent(bubbleText);
      toolLockUntil = now + 1200;
    }
    return;
  }

  // --- waiting: persistent bubble ---
  if (coreSignal === 'waiting') {
    if (bubbleText && bubbleText !== lastBubble) {
      lastBubble = bubbleText;
      bubble.showPersistent(bubbleText);
    }
    return;
  }

  // --- ready: short bubble, auto-hide after 4s ---
  if (coreSignal === 'ready') {
    if (now < toolLockUntil) {
      // within tool lock, keep current bubble
      return;
    }
    lastBubble = '';
    if (bubbleText) {
      bubble.show(bubbleText);
    }
    return;
  }

  // --- idle: clear after tool lock expires ---
  if (coreSignal === 'idle') {
    if (now < toolLockUntil) {
      return; // keep tool bubble
    }
    lastBubble = '';
    if (bubbleText) {
      bubble.show(bubbleText);
    } else {
      bubble.hide();
    }
    return;
  }
}

// ========== Interactive input ==========

function setupInteractiveInput() {
  const askSend = document.getElementById('ask-send');
  const askInput = document.getElementById('ask-input');

  askSend.addEventListener('click', () => {
    const value = askInput.value.trim();
    if (value) sendUserInput(value);
  });

  askInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      const value = askInput.value.trim();
      if (value) sendUserInput(value);
    }
  });
}

// ========== Confirm buttons (Phase 3) ==========

function setupConfirmButtons() {
  const btnYes = document.getElementById('ask-confirm-yes');
  const btnNo = document.getElementById('ask-confirm-no');

  btnYes.addEventListener('click', () => {
    sendUserInput('true', 'confirm');
  });

  btnNo.addEventListener('click', () => {
    sendUserInput('false', 'confirm');
  });
}

// ========== Quick menu (Phase 3) ==========

function setupQuickMenu() {
  const menu = document.getElementById('quick-menu');
  const items = menu.querySelectorAll('.quick-menu-item');

  items.forEach(btn => {
    btn.addEventListener('click', () => {
      const action = btn.dataset.action;
      bubble.hideQuickMenu();
      switch (action) {
        case 'message':
          // Open text input bubble
          bubble.showInteractive('发消息给爱弥斯...', 'text', null, '输入消息...');
          // Set up manual send that goes through the user input path
          break;
        case 'voice':
          bubble.show('语音输入暂不支持~');
          break;
        case 'cancel':
          // Already hidden above
          break;
      }
    });
  });

  // Click outside quick menu to close
  document.addEventListener('click', (e) => {
    if (!menu.classList.contains('hidden') &&
        !menu.contains(e.target) &&
        e.target !== document.getElementById('sprite')) {
      bubble.hideQuickMenu();
    }
  });
}

// ========== Send user input ==========

async function sendUserInput(value, type = 'text') {
  // C4: user-initiated message (no pending MCP input) — show directly as bubble
  if (!inputPending) {
    if (bubble) {
      lastBubble = value;
      bubble.hideInteractive();
      bubble.showPersistent(value);
    }
    return;
  }

  // MCP-initiated input: forward to backend pending-input slot
  try {
    await fetch('http://127.0.0.1:9527/api/user/input', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ value, type }),
    });
  } catch (_) {}
  if (bubble) bubble.hideInteractive();
  inputPending = false;
}

// ========== Permission helpers ==========

function exitPermission() {
  permissionPending = false;
  if (permissionTimer) { clearInterval(permissionTimer); permissionTimer = null; }
}

// ========== Idle animation ==========

function handleIdleAnimation(animation) {
  if (animation === 'idle') {
    if (!idleStart) idleStart = Date.now();
    scheduleIdleAnim();
  } else {
    idleStart = 0;
    cancelIdleAnim();
  }
}

function scheduleIdleAnim() {
  if (idleAnimTimer) return;
  idleAnimTimer = setTimeout(doIdleAnim, 15000 + Math.random() * 30000);
}

function doIdleAnim() {
  idleAnimTimer = null;
  if (!idleStart) return;
  const pick = ['jumping', 'waving', 'chatting'][Math.floor(Math.random() * 3)];
  window._petAnimator.play(pick);
  setTimeout(() => {
    if (window._petAnimator) window._petAnimator.play('idle');
    scheduleIdleAnim();
  }, 2000);
}

function cancelIdleAnim() {
  if (idleAnimTimer) { clearTimeout(idleAnimTimer); idleAnimTimer = null; }
}

document.addEventListener('DOMContentLoaded', init);
