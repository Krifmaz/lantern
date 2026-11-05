/*
 * LANTERN: the instance name in place of autobrr's logo.
 *
 * Lantern's pages show the instance name (the app_name setting, "Lantern" unless changed) as plain bold text, as
 * the kit's sidebar does. Lantern's server puts it into index.html as window.APP.instanceName; the frontend run on
 * its own (pnpm dev) shows "Lantern".
 */

/** The instance name Lantern's server gave, or "Lantern". */
function instanceName(): string {
  const name = (window.APP as { instanceName?: string } | undefined)?.instanceName;
  return name && name !== "{{.InstanceName}}" ? name : "Lantern";
}

/**
 * The name, drawn where autobrr's logo was. `className` is the logo's (its height sets the text size: h-10 in the
 * header, h-12 on the sign-in pages, h-24 and up on the error pages).
 */
export default function Wordmark({ className }: { className?: string }) {
  const size = /\bh-(24|48)\b/.test(className ?? "") ? "text-5xl" : /\bh-12\b/.test(className ?? "") ? "text-3xl" : "text-xl";
  return (
    <span className={`${className ?? ""} ${size} inline-flex items-center font-bold text-gray-900 dark:text-white`}>
      {instanceName()}
    </span>
  );
}
