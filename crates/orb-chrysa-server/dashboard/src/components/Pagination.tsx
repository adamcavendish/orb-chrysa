import { t } from "../lib/i18n";

interface PaginationProps {
  shown: number;
  total: number;
}

export default function Pagination(props: PaginationProps) {
  return (
    <div class="pagination" aria-label={t("common.pagination")}>
      <span class="pagination-summary">
        {t("repos.pagination", { shown: props.shown, total: props.total })}
      </span>
      <span class="pagination-size">{t("repos.pageSize", { size: 50 })}</span>
      <div class="pagination-controls">
        <button type="button" disabled>
          {t("common.previous")}
        </button>
        <span class="pagination-page">1</span>
        <button type="button" disabled>
          {t("common.next")}
        </button>
      </div>
    </div>
  );
}
