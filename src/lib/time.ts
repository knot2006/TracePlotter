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

/** Date of a Unix time in seconds. */
export const dateOnly = (t: number) => new Date(t * 1000).toLocaleDateString(locale);

/** Time of day of a Unix time in seconds. */
export const timeOfDay = (t: number) => new Date(t * 1000).toLocaleTimeString(locale);

/** Hours and minutes of a Unix time in seconds. */
export const hourMinute = (t: number) =>
  new Date(t * 1000).toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" });

/**
 * Labels for the ticks of a time axis. Seconds show only where a tick falls between
 * whole minutes, and once the ticks span more than one day, the first tick of each day
 * carries its date on a second line, so that a week reads as a week.
 */
export function timeTicks(ticks: number[]): string[] {
  const time = ticks.some((t) => t % 60) ? timeOfDay : hourMinute;
  const days = new Set(ticks.map(dateOnly)).size > 1;
  return ticks.map((t, i) => {
    const day = dateOnly(t);
    return days && (i === 0 || day !== dateOnly(ticks[i - 1])) ? `${time(t)}\n${day}` : time(t);
  });
}
