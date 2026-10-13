// Dates and times are written the way the host is set up to show them (Windows regional
// format), not in the webview's own language, which is usually US English.

let locale: string | undefined;

/** Sets the host's regional format, as a locale name such as "pl-PL". Ignored if unusable. */
export function setLocale(name: string | null) {
  try {
    if (name) new Intl.DateTimeFormat(name);
    locale = name ?? undefined;
  } catch {
    // An unknown name: keep the webview's default.
  }
}

/** Date and time of a Unix time in seconds. */
export const dateTime = (t: number) => new Date(t * 1000).toLocaleString(locale);

/** Time of day of a Unix time in seconds. */
export const timeOfDay = (t: number) => new Date(t * 1000).toLocaleTimeString(locale);
