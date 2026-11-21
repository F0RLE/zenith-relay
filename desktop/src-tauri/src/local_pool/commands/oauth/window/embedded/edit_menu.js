(() => {
  // Provider content gets no Tauri commands or clipboard access.
  if (window.top !== window || window.__relaySignInEdit) return;
  const textTypes = new Set(['text', 'password', 'search', 'tel', 'url', 'email']);
  let context = null;
  const writable = (field) => field && !field.disabled && !field.readOnly;
  const read = () => {
    if (!context || !context.target.isConnected) return null;
    if (context.field) {
      if (context.field.value !== context.value) return null;
      return context.start === null ? context.text : context.value.slice(context.start, context.end);
    }
    return context.range?.toString() === context.text ? context.text : null;
  };
  const replace = (value) => {
    const { field, start, end, range, target } = context;
    if (read() === null || (field ? !writable(field) : !target.isContentEditable)) return false;
    if (field) {
      if (start === null) {
        field.focus();
        if (window.getSelection()?.toString() !== context.text) return false;
        return document.execCommand(value ? 'insertText' : 'delete', false, value);
      }
      const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(field), 'value')?.set;
      if (!setter) return false;
      field.focus();
      setter.call(field, field.value.slice(0, start) + value + field.value.slice(end));
      if (field.selectionStart !== null) field.setSelectionRange(start + value.length, start + value.length);
      field.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: value ? 'insertText' : 'deleteContentBackward', data: value }));
      return true;
    }
    if (!range) return false;
    target.focus();
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    return document.execCommand(value ? 'insertText' : 'delete', false, value);
  };
  Object.defineProperty(window, '__relaySignInEdit', { value: ({ token, action, value = '' }) => {
    if (!context || token !== context.token) return { ok: false };
    if (action === 'dismiss') { context = null; return { ok: true }; }
    if (action === 'read') {
      const text = read();
      return text === null || text.length > 1_048_576 ? { ok: false } : { ok: true, text };
    }
    let ok = false;
    if (action === 'selectAll') {
      if (context.field && !context.field.disabled) {
        context.field.focus();
        context.field.select();
        ok = true;
      } else {
        const range = document.createRange();
        range.selectNodeContents(context.target.isContentEditable ? context.target : document.body);
        const selection = window.getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
        ok = true;
      }
    } else if (action === 'paste' && typeof value === 'string' && value.length <= 1_048_576) ok = replace(value);
    else if (action === 'cut') ok = replace('');
    context = null;
    return { ok };
  } });
  document.addEventListener('contextmenu', (event) => {
    if (!event.isTrusted || !(event.target instanceof Element)) return;
    const target = event.target.closest('input, textarea, [contenteditable="true"], [contenteditable=""]') ?? event.target;
    const field = target instanceof HTMLTextAreaElement || (target instanceof HTMLInputElement && textTypes.has(target.type)) ? target : null;
    const selection = window.getSelection();
    const range = !field && selection?.rangeCount ? selection.getRangeAt(0).cloneRange() : null;
    const start = field?.selectionStart ?? null;
    const end = field?.selectionEnd ?? null;
    const text = field && start !== null ? field.value.slice(start, end) : selection?.toString() ?? '';
    const editable = field ? writable(field) : target.isContentEditable;
    const token = crypto.randomUUID();
    context = { token, target, field, start, end, value: field?.value, range, text };
    event.preventDefault();
    if (!field && !editable && !text) { context = null; return; }
    const bounds = target.getBoundingClientRect();
    window.ipc.postMessage(JSON.stringify({
      kind: 'relayEditMenu', token,
      x: event.clientX || bounds.left + 8, y: event.clientY || bounds.top + 8,
      canCopy: Boolean(text), canCut: Boolean(text && editable),
      canPaste: Boolean(editable), canSelectAll: !field?.disabled,
    }));
  }, true);
})();
