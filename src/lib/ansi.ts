/**
 * 极简 ANSI 转义 → HTML 转换：支持 8/16 前景色、粗体、重置。
 * 终端面板性能关键路径：只做单次正则扫描，不引入重型依赖。
 */

const COLORS: Record<number, string> = {
  30: '#4b5563', 31: '#f87171', 32: '#4ade80', 33: '#facc15',
  34: '#60a5fa', 35: '#c084fc', 36: '#22d3ee', 37: '#e5e7eb',
  90: '#6b7280', 91: '#fca5a5', 92: '#86efac', 93: '#fde047',
  94: '#93c5fd', 95: '#d8b4fe', 96: '#67e8f9', 97: '#ffffff',
};

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

export function ansiToHtml(input: string): string {
  const escaped = escapeHtml(input);
  let open = false;
  const out = escaped.replace(/\x1b\[([0-9;]*)m/g, (_, codes: string) => {
    const parts = codes.split(';').map((c) => parseInt(c || '0', 10));
    let html = '';
    for (const code of parts) {
      if (code === 0) {
        if (open) { html += '</span>'; open = false; }
      } else if (code === 1) {
        if (open) html += '</span>';
        html += '<span style="font-weight:700">';
        open = true;
      } else if (COLORS[code]) {
        if (open) html += '</span>';
        html += `<span style="color:${COLORS[code]}">`;
        open = true;
      }
    }
    return html;
  });
  return open ? out + '</span>' : out;
}
