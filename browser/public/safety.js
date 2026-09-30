export function extract(value) {
  const input = value.trim();
  if (input.length > 16384) throw new Error("This transfer link is too long.");
  if (input.startsWith("blob")) return input;
  try {
    const url = new URL(input);
    if (!["https:", "http:"].includes(url.protocol) || url.pathname !== "/dropwire/receive") throw new Error();
    const ticket = decodeURIComponent(url.hash.slice(1));
    if (!ticket.startsWith("blob")) throw new Error();
    return ticket;
  } catch { throw new Error("Paste the complete Dropwire code or transfer link."); }
}
export function safeName(name) {
  let value = name.replace(/\\/g, "/").split("/").filter((part) => part && part !== "." && part !== "..").join("_").replace(/[\x00-\x1f\x7f<>:"|?*]/g, "_").replace(/[. ]+$/, "");
  if (!value) value = "file";
  if (/^(con|prn|aux|nul|conin\$|conout\$|com[0-9¹²³]|lpt[0-9¹²³])(\.|$)/i.test(value)) value = "_" + value;
  let length = 0, result = "";
  const encoder = new TextEncoder();
  for (const point of value) {
    const bytes = encoder.encode(point).length;
    if (length + bytes > 220) break;
    result += point; length += bytes;
  }
  return result || "file";
}
export function uniqueName(name, used) {
  let value = name, count = 2;
  const dot = name.lastIndexOf("."), stem = dot > 0 ? name.slice(0, dot) : name, ext = dot > 0 ? name.slice(dot) : "";
  while (used.has(value.toLowerCase())) value = `${stem} (${count++})${ext}`;
  return value;
}
