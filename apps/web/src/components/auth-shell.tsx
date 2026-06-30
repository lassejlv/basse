import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { OTPField, OTPFieldInput } from "@/components/ui/otp-field";

const OTP_LENGTH = 6;
const OTP_SLOT_KEYS = Array.from({ length: OTP_LENGTH }, (_, i) => `otp-slot-${i}`);

function BrandMark() {
  return (
    <div className="flex size-9 items-center justify-center rounded-[10px] bg-foreground font-semibold text-background text-[17px] leading-none shadow-sm ring-1 ring-black/5">
      B
    </div>
  );
}

export function AuthShell({
  title,
  subtitle,
  children,
  footer,
}: {
  title: string;
  subtitle?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
}) {
  return (
    <div className="relative flex min-h-svh flex-col items-center justify-center overflow-hidden px-4 py-12">
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 -z-10 bg-[radial-gradient(circle_at_50%_-8%,--theme(--color-foreground/6%),transparent_55%)]"
      />
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 -z-10 [background-image:linear-gradient(to_right,--theme(--color-foreground/[3%])_1px,transparent_1px),linear-gradient(to_bottom,--theme(--color-foreground/[3%])_1px,transparent_1px)] [background-size:36px_36px] [mask-image:radial-gradient(ellipse_58%_50%_at_50%_38%,black,transparent)] [-webkit-mask-image:radial-gradient(ellipse_58%_50%_at_50%_38%,black,transparent)]"
      />

      <div className="w-full max-w-sm">
        <div className="mb-7 flex flex-col items-center text-center">
          <BrandMark />
          <h1 className="mt-5 text-balance font-semibold text-xl tracking-tight">{title}</h1>
          {subtitle ? (
            <p className="mt-1.5 text-balance text-muted-foreground text-sm">{subtitle}</p>
          ) : null}
        </div>

        <div className="rounded-xl border bg-card p-6 shadow-sm">{children}</div>

        {footer ? (
          <div className="mt-6 text-center text-muted-foreground text-sm">{footer}</div>
        ) : null}
      </div>
    </div>
  );
}

export function OtpEntry({
  value,
  onValueChange,
  onComplete,
  pending,
  error,
  onResend,
  resending,
  submitLabel = "Continue",
}: {
  value: string;
  onValueChange: (value: string) => void;
  onComplete: (code: string) => void;
  pending: boolean;
  error: string | null;
  onResend: () => void;
  resending: boolean;
  submitLabel?: string;
}) {
  function handleChange(next: string) {
    onValueChange(next);
    if (next.length === OTP_LENGTH && !pending) {
      onComplete(next);
    }
  }

  return (
    <div className="space-y-4">
      <div className="flex justify-center">
        <OTPField
          aria-label="Verification code"
          length={OTP_LENGTH}
          value={value}
          onValueChange={handleChange}
          disabled={pending}
          size="lg"
        >
          {OTP_SLOT_KEYS.map((slotKey, index) => (
            <OTPFieldInput
              key={slotKey}
              aria-label={index === 0 ? undefined : `Character ${index + 1} of ${OTP_LENGTH}`}
            />
          ))}
        </OTPField>
      </div>

      {error ? <p className="text-center text-destructive-foreground text-sm">{error}</p> : null}

      <Button
        className="w-full"
        loading={pending}
        disabled={value.length !== OTP_LENGTH}
        type="button"
        onClick={() => onComplete(value)}
      >
        {submitLabel}
      </Button>

      <button
        type="button"
        className="w-full text-center text-muted-foreground text-sm transition-colors hover:text-foreground disabled:opacity-50"
        disabled={resending}
        onClick={onResend}
      >
        {resending ? "Sending a new code…" : "Didn’t get it? Resend code"}
      </button>
    </div>
  );
}
