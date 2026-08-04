/**
 * 检查更新：连接 GitHub Releases API（仓库需公开）。
 * 有新版 Release 时返回最新版本信息，供「关于」页提示用户下载。
 */

/** GitHub 公开仓库（创建后确认仓库名，若不同改这里即可） */
export const GITHUB_REPO = 'mingwx1204/clawdesk';

export interface UpdateCheckResult {
  available: boolean;
  latest?: string;
  url?: string;
  notes?: string;
  error?: string;
}

/** 比较语义化版本 a > b 返回 1，a < b 返回 -1，相等返回 0 */
function compareVersions(a: string, b: string): number {
  const pa = a.split('.').map((n) => parseInt(n, 10) || 0);
  const pb = b.split('.').map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const na = pa[i] ?? 0;
    const nb = pb[i] ?? 0;
    if (na !== nb) return na > nb ? 1 : -1;
  }
  return 0;
}

/** 检查是否有新版本 */
export async function checkForUpdates(currentVersion: string): Promise<UpdateCheckResult> {
  try {
    const resp = await fetch(
      `https://api.github.com/repos/${GITHUB_REPO}/releases/latest`,
      { headers: { Accept: 'application/vnd.github+json' } },
    );
    if (resp.status === 404) {
      return { available: false, error: '仓库或 Release 不存在（尚未发布版本）' };
    }
    if (!resp.ok) {
      return { available: false, error: `检查失败 (HTTP ${resp.status})` };
    }
    const data = await resp.json();
    const latest = String(data.tag_name || '').replace(/^v/i, '');
    if (!latest) {
      return { available: false, error: '未找到版本信息' };
    }
    if (compareVersions(latest, currentVersion) > 0) {
      return {
        available: true,
        latest,
        url: data.html_url,
        notes: typeof data.body === 'string' ? data.body.slice(0, 400) : undefined,
      };
    }
    return { available: false };
  } catch (e) {
    return {
      available: false,
      error: `网络错误: ${e instanceof Error ? e.message : String(e)}`,
    };
  }
}
