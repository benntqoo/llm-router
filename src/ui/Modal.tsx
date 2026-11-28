import { useEffect, type ReactNode } from "react";

/**
 * 通用模态框（自写，**不引第三方依赖**；02 §6.8.5）。
 *
 * 用法：`<Modal title=... onClose=... footer=...>`。
 * 点遮罩或按 `Esc` 都会调 `onClose` → **取消时不留任何副作用**
 * （T24 的关键语义：取消确认 = 渠道保持关闭）。
 */
export default function Modal(props: {
  /** 标题。 */
  title: string;
  /** 关闭（点遮罩 / 按 Esc / 点取消）。 */
  onClose: () => void;
  /** 主体。 */
  children: ReactNode;
  /** 底部按钮区。 */
  footer?: ReactNode;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        props.onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [props.onClose]);

  return (
    <div className="modal-overlay" role="presentation" onClick={props.onClose}>
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label={props.title}
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="modal-title">{props.title}</h3>
        <div className="modal-body">{props.children}</div>
        <div className="modal-footer">{props.footer}</div>
      </div>
    </div>
  );
}
