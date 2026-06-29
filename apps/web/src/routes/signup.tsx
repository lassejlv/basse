import { useMutation } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { FormEvent, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { authClient } from "@/lib/auth-client";

export const Route = createFileRoute("/signup")({
  component: SignupRoute,
});

function SignupRoute() {
  const navigate = useNavigate();
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);

  const signupMutation = useMutation({
    mutationFn: async () => {
      setError(null);
      const result = await authClient.signUp.email({
        name,
        email,
        password,
        callbackURL: "/dashboard",
      });

      if (result.error) {
        throw new Error(result.error.message ?? "Could not create account");
      }

      return result.data;
    },
    onSuccess: () => navigate({ to: "/dashboard" }),
    onError: (mutationError) => setError(mutationError.message),
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    signupMutation.mutate();
  }

  return (
    <section className="mx-auto w-full max-w-md rounded-lg border bg-card p-6">
      <div>
        <h2 className="text-xl font-semibold">Create account</h2>
        <p className="mt-2 text-sm text-muted-foreground">
          Start deploying apps from your own servers.
        </p>
      </div>

      <form className="mt-6 space-y-4" onSubmit={handleSubmit}>
        <div className="space-y-2">
          <Label htmlFor="signup-name">Name</Label>
          <Input
            id="signup-name"
            autoComplete="name"
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

        {error ? <p className="text-sm text-destructive-foreground">{error}</p> : null}

        <Button className="w-full" loading={signupMutation.isPending} type="submit">
          Sign up
        </Button>
      </form>

      <p className="mt-5 text-center text-sm text-muted-foreground">
        Already have an account?{" "}
        <Link to="/login" className="font-medium text-foreground hover:underline">
          Login
        </Link>
      </p>
    </section>
  );
}
