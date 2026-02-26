# GitHub Copilot Setup Steps

This repository is configured to work with GitHub Copilot's coding agent. The agent uses an ephemeral development environment powered by GitHub Actions to explore code, make changes, execute tests, and run linters.

## Copilot Environment Configuration

The Copilot coding agent's environment is pre-configured via the workflow file at `.github/workflows/copilot-setup-steps.yml`. This workflow:

- **Automatically installs** required tools and dependencies before Copilot starts working
- **Supports multiple languages** including .NET, Python, Node.js, and Terraform
- **Runs automatically** when changes are made to the workflow file for easy validation
- **Can be manually tested** from the repository's "Actions" tab

## What's Pre-installed

The Copilot environment includes the following tools and runtimes:

- **.NET SDK 10.0.x** - For building and testing .NET projects
- **Python 3.x** - With automatic `pip install` from `requirements.txt` if present
- **Node.js 20** - With automatic `npm ci` from `package.json` if present
- **Terraform 1.5.x** - For infrastructure as code operations

All dependency installation steps are conditional and will only run if the corresponding project files exist (e.g., `*.sln`, `*.csproj`, `requirements.txt`, `package.json`).

## Customizing the Environment

To customize Copilot's environment for your specific needs:

1. **Edit the workflow file**: `.github/workflows/copilot-setup-steps.yml`
2. **Add or modify steps** in the `copilot-setup-steps` job
3. **Test your changes** by:
   - Creating a pull request with your changes (the workflow will run automatically)
   - Manually running the workflow from the repository's "Actions" tab

### Customizable Settings

You can customize the following aspects of the `copilot-setup-steps` job:

- `steps` - The setup steps to run before the agent starts
- `permissions` - The minimal permissions needed for your setup steps
- `runs-on` - The runner to use (e.g., `ubuntu-latest`, or larger runners)
- `services` - Any service containers needed during setup
- `timeout-minutes` - Maximum time for setup (Copilot enforces a maximum of 59 minutes, as documented in the [workflow syntax](https://docs.github.com/en/copilot/how-tos/use-copilot-agents/coding-agent/customize-the-agent-environment))

### Example: Adding a New Tool

To add a new tool or runtime, add a step to the workflow:

```yaml
- name: Set up Go
  uses: actions/setup-go@v5
  with:
    go-version: '1.21'
```

## Setting Environment Variables

To set environment variables for Copilot, create GitHub Actions variables or secrets in the `copilot` environment:

1. Go to your repository **Settings** → **Environments**
2. Click the `copilot` environment (create it if it doesn't exist)
3. Add environment secrets or variables as needed

For sensitive values like API keys, use environment secrets instead of variables.

## Using Larger Runners

By default, Copilot uses standard GitHub-hosted runners. If you need more resources (RAM, CPU, disk space):

1. Set up larger runners for your repository (see [GitHub documentation](https://docs.github.com/en/actions/using-github-hosted-runners/using-larger-runners))
2. Update the `runs-on` setting in the workflow:

```yaml
copilot-setup-steps:
  runs-on: ubuntu-4-core  # or your larger runner label
```

**Note**: Copilot coding agent is only compatible with Ubuntu x64 Linux runners.

## Troubleshooting

### Setup Step Failures

If a setup step fails:
- Check the workflow run logs in the "Actions" tab
- Fix the issue in your workflow file
- Create a PR to test the changes (the workflow will run automatically)

If any setup step fails with a non-zero exit code, Copilot will skip remaining steps and begin working with the current environment state.

### Testing the Workflow

To manually test the workflow:
1. Go to the repository's **Actions** tab
2. Select **Copilot Setup Steps** workflow
3. Click **Run workflow**
4. Review the logs to ensure all steps complete successfully

## Additional Resources

- [GitHub Copilot Documentation](https://docs.github.com/en/copilot)
- [Customizing the Copilot Agent Environment](https://docs.github.com/en/copilot/how-tos/use-copilot-agents/coding-agent/customize-the-agent-environment)
- [GitHub Actions Workflow Syntax](https://docs.github.com/en/actions/using-workflows/workflow-syntax-for-github-actions)
- [Tracking Copilot's Sessions](https://docs.github.com/en/copilot/how-tos/agents/copilot-coding-agent/tracking-copilots-sessions)
