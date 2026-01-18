# Publishing Guide

## 1. Create a GitHub Release

The GitHub Actions workflow is configured to automatically build binaries when you push a version tag.

1. Commit all changes:

    ```bash
    git add .
    git commit -m "Prepare v0.1.0 release"
    ```

2. Push the tag:

    ```bash
    git tag v0.1.0
    git push origin v0.1.0
    ```

3. Wait for the "Release" action to complete on GitHub. It will create a Draft Release or a Release with the binaries attached (`code-intelligence-mcp-server-*.tar.gz`).
    * **Verify** that the release exists and has the assets attached before proceeding to step 2.

## 2. Publish to NPM

Once the binaries are available on GitHub, you can publish the NPM wrapper.

1. Navigate to the `npm` directory:

    ```bash
    cd npm
    ```

2. Install dependencies (to generate lockfile, optional but good practice):

    ```bash
    npm install
    ```

3. Publish:

    ```bash
    npm publish --access public
    ```

    *(You need to be logged into npm via `npm login` first)*

## 3. Usage for Users

Once published, users can use your MCP server without installing anything manually:

**OpenCode Config (`opencode.json`):**

```json
{
  "mcp": {
    "code-intelligence": {
      "type": "local",
      "command": ["npx", "-y", "code-intelligence-mcp"],
      "env": {}
    }
  }
}
```

The wrapper script automatically:

* Sets `BASE_DIR` to the current project root.
* Enables the `candle` local AI backend.
* Enables Metal acceleration on macOS.
* Stores indexes in `.cimcp/` inside the project.
