# Protects the default branch with a GitHub ruleset. Run once after the repo is
# public (private repos need GitHub Pro for rulesets). Needs the gh CLI, signed in
# as a repo admin. Running it again updates the existing ruleset.
#
#   powershell -ExecutionPolicy Bypass -File tools\protect-main.ps1
#
# What it does on the default branch:
# - Changes only through pull requests (no approvals required: a solo owner
#   can't approve their own PRs).
# - The CI check ("check" from .github/workflows/ci.yml, reported by GitHub
#   Actions) must pass.
# - No force pushes, no deleting the branch.
# - Repo admins may merge a PR whose checks are missing (older stacked PRs
#   predate CI), but they can't push to the branch directly either.
param([string] $Repo = 'mdi1984/graft', [switch] $DryRun)
$ErrorActionPreference = 'Stop'

$name = 'Protect main'
$ruleset = @{
  name          = $name
  target        = 'branch'
  enforcement   = 'active'
  conditions    = @{ ref_name = @{ include = @('~DEFAULT_BRANCH'); exclude = @() } }
  # Actor 5 is the built-in "Repository admin" role.
  bypass_actors = @(@{ actor_id = 5; actor_type = 'RepositoryRole'; bypass_mode = 'pull_request' })
  rules         = @(
    @{ type = 'deletion' },
    @{ type = 'non_fast_forward' },
    @{
      type       = 'pull_request'
      parameters = @{
        required_approving_review_count   = 0
        dismiss_stale_reviews_on_push     = $false
        require_code_owner_review         = $false
        require_last_push_approval        = $false
        required_review_thread_resolution = $false
      }
    },
    @{
      type       = 'required_status_checks'
      parameters = @{
        strict_required_status_checks_policy = $false
        # 15368 is the GitHub Actions app: only Actions can report this check.
        required_status_checks               = @(@{ context = 'check'; integration_id = 15368 })
      }
    }
  )
}

$json = $ruleset | ConvertTo-Json -Depth 10
if ($DryRun) { $json; return }

$body = New-TemporaryFile
try {
  # gh rejects a byte order mark, which Set-Content would add.
  [IO.File]::WriteAllText($body.FullName, $json, (New-Object Text.UTF8Encoding $false))
  $all = gh api "repos/$Repo/rulesets"
  if ($LASTEXITCODE -ne 0) { throw "Could not read the rulesets of $Repo (is it public, and are you an admin?)." }
  $existing = ($all | ConvertFrom-Json | Where-Object { $_.name -eq $name }).id
  if ($existing) {
    gh api --method PUT "repos/$Repo/rulesets/$existing" --input $body | Out-Null
  } else {
    gh api --method POST "repos/$Repo/rulesets" --input $body | Out-Null
  }
  if ($LASTEXITCODE -ne 0) { throw 'GitHub refused the ruleset.' }
  Write-Host "Main is protected on $Repo (ruleset '$name')."
} finally {
  Remove-Item $body -ErrorAction SilentlyContinue
}
