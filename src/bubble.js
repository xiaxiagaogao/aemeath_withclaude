class Bubble {
  constructor(el) {
    this.el = el;
    this.queue = [];
    this.displayTimer = null;
    this.displayMs = 4000;
    this.pendingPollTimer = null;
  }

  show(text) {
    if (!text) {
      this.hide();
      return;
    }

    // If bubble is already visible, enqueue
    if (
      !this.el.classList.contains('hidden') &&
      !this.el.classList.contains('fade-out')
    ) {
      this.queue.push(text);
      return;
    }

    this._display(text);
  }

  /** Show without auto-hide timer (persistent bubble for tools/waiting) */
  showPersistent(text) {
    if (!text) {
      this.hide();
      return;
    }
    this._display(text);
    // Cancel auto-hide so bubble stays until explicitly cleared
    if (this.displayTimer) {
      clearTimeout(this.displayTimer);
      this.displayTimer = null;
    }
  }

  _display(text) {
    this.el.textContent = text;
    this.el.classList.remove('hidden', 'fade-out');
    this.el.classList.add('visible');

    if (this.displayTimer) clearTimeout(this.displayTimer);

    this.displayTimer = setTimeout(() => {
      this.el.classList.add('fade-out');
      setTimeout(() => {
        this.el.classList.add('hidden');
        this.el.classList.remove('visible', 'fade-out');
        if (this.queue.length > 0) {
          const next = this.queue.shift();
          this._display(next);
        }
      }, 400);
    }, this.displayMs);
  }

  hide() {
    this.el.classList.add('hidden');
    this.el.classList.remove('visible', 'fade-out');
    this.queue = [];
    if (this.displayTimer) {
      clearTimeout(this.displayTimer);
      this.displayTimer = null;
    }
  }

  // ========== Interactive bubble methods (Phase 1 + Phase 3) ==========

  /** Show interactive bubble: routes to correct UI based on input_type */
  showInteractive(prompt, inputType, options, placeholder) {
    const askBubble = document.getElementById('ask-bubble');
    const askPrompt = document.getElementById('ask-prompt');
    const askInput = document.getElementById('ask-input');
    const askInputRow = document.getElementById('ask-input-row');
    const askChoices = document.getElementById('ask-choices');
    const askConfirmRow = document.getElementById('ask-confirm-row');

    askPrompt.textContent = prompt;
    askInput.value = '';

    // Reset all sub-sections
    askInputRow.classList.add('hidden');
    askChoices.innerHTML = '';
    askChoices.classList.add('hidden');
    askConfirmRow.classList.add('hidden');

    switch (inputType) {
      case 'confirm':
        askConfirmRow.classList.remove('hidden');
        break;
      case 'select':
        if (options && options.length > 0) {
          askChoices.classList.remove('hidden');
          for (const opt of options) {
            const btn = document.createElement('button');
            btn.textContent = opt;
            btn.addEventListener('click', () => {
              window._sendUserInput(opt, 'select');
            });
            askChoices.appendChild(btn);
          }
        }
        break;
      case 'text':
      default:
        askInputRow.classList.remove('hidden');
        if (placeholder) askInput.placeholder = placeholder;
        // Auto-focus after DOM update
        setTimeout(() => askInput.focus(), 50);
        break;
    }

    askBubble.classList.remove('hidden');

    // Hide the regular text bubble
    this.hide();
  }

  /** Show interactive bubble with text input (legacy, delegates) */
  showWithInput(prompt, placeholder) {
    this.showInteractive(prompt, 'text', null, placeholder || '输入...');
  }

  /** Show interactive bubble with option buttons (legacy, delegates) */
  showWithOptions(prompt, options) {
    this.showInteractive(prompt, 'select', options, null);
  }

  /** Hide interactive bubble, restore normal bubble */
  hideInteractive() {
    const askBubble = document.getElementById('ask-bubble');
    const askInput = document.getElementById('ask-input');
    const askChoices = document.getElementById('ask-choices');
    const askConfirmRow = document.getElementById('ask-confirm-row');
    const askInputRow = document.getElementById('ask-input-row');
    askBubble.classList.add('hidden');
    askInput.value = '';
    askChoices.innerHTML = '';
    askChoices.classList.add('hidden');
    askConfirmRow.classList.add('hidden');
    askInputRow.classList.remove('hidden'); // default
    this.stopPendingPoll();
  }

  // ========== Quick menu (click sprite) ==========

  /** Show quick menu bubble on click */
  showQuickMenu() {
    // Don't show if interactive bubble is already open
    const askBubble = document.getElementById('ask-bubble');
    if (!askBubble.classList.contains('hidden')) return;

    const menu = document.getElementById('quick-menu');
    menu.classList.remove('hidden');
    // Auto-hide after 5s
    setTimeout(() => {
      if (!menu.classList.contains('hidden')) menu.classList.add('hidden');
    }, 5000);
  }

  /** Hide quick menu */
  hideQuickMenu() {
    const menu = document.getElementById('quick-menu');
    menu.classList.add('hidden');
  }

  // ========== Pending indicator ==========

  /** Start polling /api/user/pending to detect backend waiting for input */
  startPendingPoll() {
    if (this.pendingPollTimer) return;
    const dot = document.getElementById('pending-dot');
    dot.classList.remove('hidden');

    this.pendingPollTimer = setInterval(async () => {
      try {
        const r = await fetch('http://127.0.0.1:9527/api/user/pending');
        if (r.ok) {
          const data = await r.json();
          if (data.waiting && !document.getElementById('ask-bubble').classList.contains('hidden')) {
            dot.classList.remove('hidden');
          } else {
            dot.classList.add('hidden');
          }
        }
      } catch (_) {
        dot.classList.add('hidden');
      }
    }, 2000);
  }

  /** Stop pending polling */
  stopPendingPoll() {
    if (this.pendingPollTimer) {
      clearInterval(this.pendingPollTimer);
      this.pendingPollTimer = null;
    }
    document.getElementById('pending-dot').classList.add('hidden');
  }
}

window.Bubble = Bubble;
