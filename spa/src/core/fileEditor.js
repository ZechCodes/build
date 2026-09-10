import "../styles/fileEditor.css";

export function mountFileEditor(host, { value, selection, onEdit, onSelection } = {}) {
  const input = document.createElement("textarea");
  input.className = "file-editor";
  input.spellcheck = false;
  input.autocapitalize = "off";
  input.autocomplete = "off";
  input.setAttribute("autocorrect", "off");
  input.value = value || "";
  host.replaceChildren(input);
  const reportSelection = () => onSelection?.({ start: input.selectionStart, end: input.selectionEnd });
  input.oninput = () => onEdit?.(input.value, { start: input.selectionStart, end: input.selectionEnd });
  input.onselect = reportSelection;
  input.setSelectionRange(selection?.start || 0, selection?.end || 0);
  input.focus();
  return {
    focus: () => input.focus(),
    selection: () => ({ start: input.selectionStart, end: input.selectionEnd }),
    dispose() {
      input.oninput = null;
      input.onselect = null;
    },
  };
}
