import { t } from "../lib/i18n";

export default function NotFound() {
  return (
    <div style="text-align:center;padding:4rem 1rem">
      <h1>404</h1>
      <p>{t("notFound.message")}</p>
      <a href="/" style="color:var(--color-accent)">
        {t("notFound.back")}
      </a>
    </div>
  );
}
