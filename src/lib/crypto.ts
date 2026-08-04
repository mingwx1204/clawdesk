/**
 * 敏感数据加解密（Web Crypto API, AES-GCM）。
 * API Key 加密后存入 settings 表，运行时可解密，但磁盘上不可直接读取。
 */

let cryptoKey: CryptoKey | null = null;

/** 从本地存储获取或生成加密密钥 */
async function getOrCreateKey(): Promise<CryptoKey> {
  if (cryptoKey) return cryptoKey;

  const STORAGE_KEY = 'clawdesk-crypto-key-v1';
  const raw = localStorage.getItem(STORAGE_KEY);

  if (raw) {
    try {
      const jwk = JSON.parse(raw) as JsonWebKey;
      cryptoKey = await crypto.subtle.importKey(
        'jwk', jwk, { name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt'],
      );
      return cryptoKey!;
    } catch { /* 损坏则重新生成 */ }
  }

  // 生成新密钥
  cryptoKey = await crypto.subtle.generateKey(
    { name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt'],
  );

  // 导出并持久化
  const jwk = await crypto.subtle.exportKey('jwk', cryptoKey);
  localStorage.setItem(STORAGE_KEY, JSON.stringify(jwk));

  return cryptoKey;
}

/** 加密文本，返回 base64 格式的密文（含 iv） */
export async function encrypt(plaintext: string): Promise<string> {
  const key = await getOrCreateKey();
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const encoded = new TextEncoder().encode(plaintext);
  const ciphertext = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv }, key, encoded,
  );
  // 组合 iv + ciphertext 后 base64
  const combined = new Uint8Array(iv.length + ciphertext.byteLength);
  combined.set(iv, 0);
  combined.set(new Uint8Array(ciphertext), iv.length);
  return btoa(String.fromCharCode(...combined));
}

/** 解密 base64 格式密文 */
export async function decrypt(ciphertextB64: string): Promise<string> {
  const key = await getOrCreateKey();
  const combined = Uint8Array.from(atob(ciphertextB64), (c) => c.charCodeAt(0));
  const iv = combined.slice(0, 12);
  const data = combined.slice(12);
  const decrypted = await crypto.subtle.decrypt(
    { name: 'AES-GCM', iv }, key, data,
  );
  return new TextDecoder().decode(decrypted);
}

/** 加密整个 apiKeys 对象，key → encrypted-key 映射 */
export async function encryptApiKeys(apiKeys: Record<string, string>): Promise<Record<string, string>> {
  const result: Record<string, string> = {};
  for (const [k, v] of Object.entries(apiKeys)) {
    if (v) result[k] = await encrypt(v);
  }
  return result;
}

/** 解密 apiKeys 对象 */
export async function decryptApiKeys(encrypted: Record<string, string>): Promise<Record<string, string>> {
  const result: Record<string, string> = {};
  for (const [k, v] of Object.entries(encrypted)) {
    if (v) {
      try {
        result[k] = await decrypt(v);
        // 防双重加密：若解出结果仍是密文（可二次解密），取最内层明文
        try {
          const second = await decrypt(result[k]);
          result[k] = second;
        } catch {
          // 单层加密，保留第一次解密结果
        }
      } catch {
        // 解密失败（密钥不匹配等）：不保留密文当 key，置空让用户重填
        result[k] = '';
      }
    }
  }
  return result;
}
