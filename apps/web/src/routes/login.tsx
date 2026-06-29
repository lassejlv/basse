import { useMutation } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { FormEvent, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { authClient } from "@/lib/auth-client";

export const Route = createFileRoute("/login")({
  component: LoginRoute,
});

function LoginRoute() {
  const navigate = useNavigate();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);

  const loginMutation = useMutation({
    mutationFn: async () => {
      setError(null);
      const result = await authClient.signIn.email({
        email,
        password,
        callbackURL: "/dashboard",
      });

      if (result.error) {
        throw new Error(result.error.message ?? "Could not log in");
      }

      return result.data;
    },
    onSuccess: () => navigate({ to: "/dashboard" }),
    onError: (mutationError) => setError(mutationError.message),
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    loginMutation.mutate();
  }

  return (
    <section className="mx-auto w-full max-w-md rounded-lg border bg-card p-6">
      <div>
        <h2 className="text-xl font-semibold">Login</h2>
        <p className="mt-2 text-sm text-muted-foreground">Continue to your Basse account.</p>
      </div>

      <form className="mt-6 space-y-4" onSubmit={handleSubmit}>
        <div className="space-y-2">
          <Label htmlFor="login-email">Email</Label>
          <Input
            id="login-email"
            type="email"
            autoComplete="email"
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

        {error ? <p className="text-sm text-destructive-foreground">{error}</p> : null}

        <Button className="w-full" loading={loginMutation.isPending} type="submit">
          Login
        </Button>
      </form>

      <p className="mt-5 text-center text-sm text-muted-foreground">
        No account yet?{" "}
        <Link to="/signup" className="font-medium text-foreground hover:underline">
          Sign up
        </Link>
      </p>
    </section>
  );
}
