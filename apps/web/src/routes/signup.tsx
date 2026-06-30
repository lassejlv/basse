import { useMutation } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { type FormEvent, useState } from "react";
import { AuthShell, OtpEntry } from "@/components/auth-shell";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { authClient } from "@/lib/auth-client";

export const Route = createFileRoute("/signup")({
  component: SignupRoute,
});

function SignupRoute() {
  const navigate = useNavigate();
  const [mode, setMode] = useState<"form" | "verify">("form");
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [otp, setOtp] = useState("");
  const [error, setError] = useState<string | null>(null);

  const sendOtpMutation = useMutation({
    mutationFn: async () => {
      setError(null);
      const result = await authClient.emailOtp.sendVerificationOtp({
        email,
        type: "email-verification",
      });
      if (result.error) {
        throw new Error(result.error.message ?? "Could not send a code");
      }
    },
    onSuccess: () => {
      setOtp("");
      setMode("verify");
    },
    onError: (mutationError) => setError(mutationError.message),
  });

  const signupMutation = useMutation({
    mutationFn: async () => {
      setError(null);
      const result = await authClient.signUp.email({ name, email, password });
      if (result.error) {
        throw new Error(result.error.message ?? "Could not create account");
      }
      return result.data;
    },
    onSuccess: () => sendOtpMutation.mutate(),
    onError: (mutationError) => setError(mutationError.message),
  });

  const verifyMutation = useMutation({
    mutationFn: async (code: string) => {
      setError(null);
      const verified = await authClient.emailOtp.verifyEmail({ email, otp: code });
      if (verified.error) {
        throw new Error(verified.error.message ?? "Invalid or expired code");
      }
      const signedIn = await authClient.signIn.email({ email, password });
      if (signedIn.error) {
        throw new Error(signedIn.error.message ?? "Could not sign in");
      }
    },
    onSuccess: () => navigate({ to: "/dashboard" }),
    onError: (mutationError) => {
      setError(mutationError.message);
      setOtp("");
    },
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    signupMutation.mutate();
  }

  if (mode === "verify") {
    return (
      <AuthShell
        title="Verify your email"
        subtitle={
          <>
            We sent a 6-digit code to <span className="font-medium text-foreground">{email}</span>.
          </>
        }
        footer={
          <button
            type="button"
            className="hover:text-foreground"
            onClick={() => {
              setError(null);
              setOtp("");
              setMode("form");
            }}
          >
            Use a different email
          </button>
        }
      >
        <OtpEntry
          value={otp}
          onValueChange={setOtp}
          onComplete={(code) => verifyMutation.mutate(code)}
          pending={verifyMutation.isPending}
          error={error}
          onResend={() => sendOtpMutation.mutate()}
          resending={sendOtpMutation.isPending}
          submitLabel="Verify email"
        />
      </AuthShell>
    );
  }

  return (
    <AuthShell
      title="Create your account"
      subtitle="Deploy apps from your own servers in minutes."
      footer={
        <>
          Already have an account?{" "}
          <Link to="/login" className="font-medium text-foreground hover:underline">
            Sign in
          </Link>
        </>
      }
    >
      <form className="space-y-4" onSubmit={handleSubmit}>
        <div className="space-y-2">
          <Label htmlFor="signup-name">Name</Label>
          <Input
            id="signup-name"
            autoComplete="name"
            autoFocus
            value={name}
            onChange={(event) => setName(event.currentTarget.value)}
            required
          />
        </div>
        <div className="space-y-2">
          <Label htmlFor="signup-email">Email</Label>
          <Input
            id="signup-email"
            type="email"
            autoComplete="email"
            value={email}
            onChange={(event) => setEmail(event.currentTarget.value)}
            required
          />
        </div>
        <div className="space-y-2">
          <Label htmlFor="signup-password">Password</Label>
          <Input
            id="signup-password"
            type="password"
            autoComplete="new-password"
            minLength={8}
            value={password}
            onChange={(event) => setPassword(event.currentTarget.value)}
            required
          />
        </div>

        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}

        <Button
          className="w-full"
          loading={signupMutation.isPending || sendOtpMutation.isPending}
          type="submit"
        >
          Create account
        </Button>
      </form>
    </AuthShell>
  );
}
