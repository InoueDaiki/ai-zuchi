/** https のリンクだけを通す。認証情報つきの URL は見た目をごまかせるので通さない。 */
export function safeHttpsUrl(url: string | null): string | null {
  if (url === null) return null;
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "https:") return null;
    if (parsed.username !== "" || parsed.password !== "") return null;
    return parsed.href;
  } catch {
    return null;
  }
}

/** 題名は外部の文字列なので、実際の行き先を並べて見せる。 */
export function hostOf(url: string): string {
  return new URL(url).hostname;
}

/** 例: 10/20 14:05 */
export function formatTime(ms: number): string {
  const at = new Date(ms);
  const minutes = String(at.getMinutes()).padStart(2, "0");
  return `${at.getMonth() + 1}/${at.getDate()} ${at.getHours()}:${minutes}`;
}
