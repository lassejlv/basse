import type { GitHubRepository } from "@basse/shared";
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

  return (
    <div className="space-y-2">
      <Label>{label}</Label>
      <Select
        value={selected?.cloneUrl ?? ""}
        onValueChange={(next) => {
          const repository = repositories.find((candidate) => candidate.cloneUrl === next);
          if (repository) onSelect(repository);
        }}
      >
        <SelectTrigger className="w-full">
          <SelectValue placeholder="Select installed repository">
            {(selectedValue: string) =>
              repositories.find((repository) => repository.cloneUrl === selectedValue)?.fullName ??
              "Select installed repository"
            }
          </SelectValue>
        </SelectTrigger>
        <SelectPopup>
          {repositories.map((repository) => (
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
