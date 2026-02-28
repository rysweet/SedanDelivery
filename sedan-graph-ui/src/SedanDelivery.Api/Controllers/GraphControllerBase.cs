using Microsoft.AspNetCore.Authorization;
using Microsoft.AspNetCore.Mvc;

namespace SedanDelivery.Api.Controllers;

/// <summary>
/// Base class for all graph controllers. Provides the shared InvalidName() helper
/// to avoid duplicating the 400 response across every controller.
/// </summary>
[Authorize]
public abstract class GraphControllerBase : ControllerBase
{
    protected IActionResult InvalidName() =>
        Problem(
            detail: "Graph name must match [a-zA-Z0-9_.-]{1,64}.",
            statusCode: StatusCodes.Status400BadRequest,
            title: "Invalid graph name");
}
