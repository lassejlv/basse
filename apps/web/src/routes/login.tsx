import { useMutation } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { type FormEvent, useState } from "react";
import { AuthShell, OtpEntry } from "@/components/auth-shell";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { authClient } from "@/lib/auth-client";

export const Route = createFileRoute("/login")({
  component: LoginRoute,
});

type Mode = "password" | "otp-email" | "otp-code";
type OtpPurpose = "sign-in" | "verify-email";
type OtpType = "sign-in" | "email-verification";

function LoginRoute() {
  const navigate = useNavigate();
  const [mode, setMode] = useState<Mode>("password");
  const [purpose, setPurpose] = useState<OtpPurpose>("sign-in");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [otp, setOtp] = useState("");
  const [error, setError] = useState<string | null>(null);

  function reset(next: Mode) {
    setError(null);
    setOtp("");
    setMode(next);
  }

  const loginMutation = useMutation({
    mutationFn: async () => {
      setError(null);
      const result = await authClient.signIn.email({ email, password });
      if (result.error) {
        // 403 = account exists but the email isn't verified yet.
        if (result.error.status === 403) {
          return { needsVerification: true as const };
        }
        throw new Error(result.error.message ?? "Could not sign in");
      }
      return { needsVerification: false as const };
    },
    onSuccess: (result) => {
      if (result.needsVerification) {
        sendOtpMutation.mutate("email-verification");
      } else {
        navigate({ to: "/dashboard" });
      }
    },
    onError: (mutationError) => setError(mutationError.message),
  });

  const sendOtpMutation = useMutation({
    mutationFn: async (type: OtpType) => {
      setError(null);
      const result = await authClient.emailOtp.sendVerificationOtp({ email, type });
      if (result.error) {
        throw new Error(result.error.message ?? "Could not send a code");
      }
      return type;
    },
    onSuccess: (type) => {
      setPurpose(type === "sign-in" ? "sign-in" : "verify-email");
      setOtp("");
      setMode("otp-code");
    },
    onError: (mutationError) => setError(mutationError.message),
  });

  const verifyMutation = useMutation({
    mutationFn: async (code: string) => {
      setError(null);
      if (purpose === "verify-email") {
        const verified = await authClient.emailOtp.verifyEmail({ email, otp: code });
        if (verified.error) {
          throw new Error(verified.error.message ?? "Invalid or expired code");
        }
        const signedIn = await authClient.signIn.email({ email, password });
        if (signedIn.error) {
          throw new Error(signedIn.error.message ?? "Could not sign in");
        }
        return;
      }
      const result = await authClient.signIn.emailOtp({ email, otp: code });
      if (result.error) {
        throw new Error(result.error.message ?? "Invalid or expired code");
      }
    },
    onSuccess: () => navigate({ to: "/dashboard" }),
    onError: (mutationError) => {
      setError(mutationError.message);
      setOtp("");
    },
  });

  function handlePasswordSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    loginMutation.mutate();
  }

  function handleSendCode(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    sendOtpMutation.mutate("sign-in");
  }

  const signupLink = (
    <>
      New to Basse?{" "}
      <Link to="/signup" className="font-medium text-foreground hover:underline">
        Create an account
      </Link>
    </>
  );

  if (mode === "otp-code") {
    return (
      <AuthShell
        title={purpose === "verify-email" ? "Verify your email" : "Enter your code"}
        subtitle={
          <>
            We sent a 6-digit code to <span className="font-medium text-foreground">{email}</span>.
          </>
        }
        footer={
          <button type="button" className="hover:text-foreground" onClick={() => reset("password")}>
            Back to sign in
          </button>
        }
      >
        <OtpEntry
          value={otp}
          onValueChange={setOtp}
          onComplete={(code) => verifyMutation.mutate(code)}
          pending={verifyMutation.isPending}
          error={error}
          onResend={() =>
            sendOtpMutation.mutate(purpose === "verify-email" ? "email-verification" : "sign-in")
          }
          resending={sendOtpMutation.isPending}
          submitLabel={purpose === "verify-email" ? "Verify email" : "Sign in"}
        />
      </AuthShell>
    );
  }

  if (mode === "otp-email") {
    return (
      <AuthShell
        title="Sign in with a code"
        subtitle="We’ll email you a one-time code — no password needed."
        footer={signupLink}
      >
        <form className="space-y-4" onSubmit={handleSendCode}>
          <div className="space-y-2">
            <Label htmlFor="otp-email">Email</Label>
            <Input
              id="otp-email"
              type="email"
              autoComplete="email"
              autoFocus
              value={email}
              onChange={(event) => setEmail(event.currentTarget.value)}
              required
            />
          </div>

          {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}

          <Button className="w-full" loading={sendOtpMutation.isPending} type="submit">
            Send code
          </Button>

          <button
            type="button"
            className="w-full text-center text-muted-foreground text-sm transition-colors hover:text-foreground"
            onClick={() => reset("password")}
          >
            Sign in with a password instead
          </button>
        </form>
      </AuthShell>
    );
  }

  return (
    <AuthShell
      title="Sign in to Basse"
      subtitle="Welcome back. Sign in to continue."
      footer={signupLink}
    >
      <form className="space-y-4" onSubmit={handlePasswordSubmit}>
        <div className="space-y-2">
          <Label htmlFor="login-email">Email</Label>
          <Input
            id="login-email"
            type="email"
            autoComplete="email"
            autoFocus
            value={email}
            onChange={(event) => setEmail(event.currentTarget.value)}
            required
          />
        </div>
        <div className="space-y-2">
          <Label htmlFor="login-password">Password</Label>
          <Input
            id="login-password"
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(event) => setPassword(event.currentTarget.value)}
            required
          />
        </div>

        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}

        <Button className="w-full" loading={loginMutation.isPending} type="submit">
          Sign in
        </Button>
      </form>

      <div className="my-5 flex items-center gap-3">
        <span className="h-px flex-1 bg-border" />
        <span className="text-muted-foreground text-xs">or</span>
        <span className="h-px flex-1 bg-border" />
      </div>

      <Button className="w-full" variant="outline" type="button" onClick={() => reset("otp-email")}>
        Email me a one-time code
      </Button>
    </AuthShell>
  );
}
