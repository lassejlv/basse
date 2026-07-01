import type { GitHubRepository } from "@basse/shared";
import { useEffect, useMemo, useState } from "react";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectItem,
  SelectPopup,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

export function GitHubRepositorySelect({
  label,
  onSelect,
  repositories,
  value,
}: {
  label: string;
  onSelect: (repository: GitHubRepository) => void;
  repositories: GitHubRepository[];
  value: string;
}) {
  const selected = repositories.find((repository) => repository.cloneUrl === value);
  const accounts = useMemo(
    () => [...new Set(repositories.map((repository) => repository.accountLogin))].sort(),
    [repositories],
  );
  const [account, setAccount] = useState(selected?.accountLogin ?? accounts[0] ?? "");
  const accountRepositories = repositories.filter(
    (repository) => repository.accountLogin === account,
  );
  const selectedInAccount =
    selected?.accountLogin === account && accountRepositories.includes(selected) ? selected : null;

  useEffect(() => {
    if (selected?.accountLogin) {
      setAccount(selected.accountLogin);
      return;
    }
    if (!account && accounts[0]) setAccount(accounts[0]);
  }, [account, accounts, selected?.accountLogin]);

  return (
    <div className="space-y-2">
      <Label>{label}</Label>
      {accounts.length > 0 ? (
        <Select value={account} onValueChange={(next) => setAccount(next ?? "")}>
          <SelectTrigger className="w-full">
            <SelectValue placeholder="Select GitHub account">
              {(selectedAccount: string) => selectedAccount || "Select GitHub account"}
            </SelectValue>
          </SelectTrigger>
          <SelectPopup>
            {accounts.map((accountLogin) => (
              <SelectItem key={accountLogin} value={accountLogin}>
                {accountLogin}
              </SelectItem>
            ))}
          </SelectPopup>
        </Select>
      ) : null}
      <Select
        value={selectedInAccount?.cloneUrl ?? ""}
        onValueChange={(next) => {
          const repository = accountRepositories.find((candidate) => candidate.cloneUrl === next);
          if (repository) onSelect(repository);
        }}
      >
        <SelectTrigger className="w-full">
          <SelectValue placeholder="Select installed repository">
            {(selectedValue: string) =>
              accountRepositories.find((repository) => repository.cloneUrl === selectedValue)
                ?.fullName ?? "Select installed repository"
            }
          </SelectValue>
        </SelectTrigger>
        <SelectPopup>
          {accountRepositories.map((repository) => (
            <SelectItem
              key={`${repository.installationId}:${repository.id}`}
              value={repository.cloneUrl}
            >
              {repository.fullName}
              {repository.private ? " · private" : ""}
            </SelectItem>
          ))}
        </SelectPopup>
      </Select>
    </div>
  );
}
