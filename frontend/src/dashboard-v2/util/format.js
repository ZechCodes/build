export const EFFORT_LABELS = {
  low: 'Low',
  medium: 'Medium',
  high: 'High',
  xhigh: 'Extra High',
  max: 'Max',
};

export function effortLabel(value) {
  return EFFORT_LABELS[value] || value;
}

export function modelToFriendlyName(modelId) {
  if (!modelId) return 'Agent';
  const m = modelId.toLowerCase();
  if (m.includes('claude')) return 'Claude';
  if (m.includes('codex')) return 'Codex';
  if (m.includes('gpt')) return 'GPT';
  if (m.includes('gemini')) return 'Gemini';
  if (m.includes('llama')) return 'Llama';
  if (m.includes('mistral')) return 'Mistral';
  if (m.includes('command')) return 'Command';
  if (m.includes('deepseek')) return 'DeepSeek';
  const first = modelId.split(/[-_\/]/)[0];
  return first.charAt(0).toUpperCase() + first.slice(1);
}

export function compareVersions(a, b) {
  const pa = a.split('.').map(Number);
  const pb = b.split('.').map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const na = pa[i] || 0;
    const nb = pb[i] || 0;
    if (na !== nb) return na - nb;
  }
  return 0;
}
