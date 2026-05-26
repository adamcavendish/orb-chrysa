import { t } from "../lib/i18n";

export default function LoadingSpinner(props: { label?: string }) {
  return <div class="spinner">{props.label ?? t("common.loading")}</div>;
}
