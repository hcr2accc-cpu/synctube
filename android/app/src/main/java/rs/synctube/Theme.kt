// Colors match the desktop app (client/src/theme.rs).
package rs.synctube

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

object Pal {
    val BG = Color(0xFF101116)
    val PANEL = Color(0xFF15171E)
    val CARD = Color(0xFF1C1F28)
    val FIELD = Color(0xFF0C0D12)
    val BORDER = Color(0xFF2A2E3A)
    val TEXT = Color(0xFFE8EAF0)
    val TEXT_DIM = Color(0xFF9AA0B0)
    val TEXT_FAINT = Color(0xFF6A7080)
    val ACCENT = Color(0xFF8B7BFF)
    val GOOD = Color(0xFF4ADE80)
    val WARN = Color(0xFFFBBF24)
    val BAD = Color(0xFFF87171)
    val ON_ACCENT = Color(0xFF0C0D12)
}

private val scheme = darkColorScheme(
    primary = Pal.ACCENT,
    onPrimary = Pal.ON_ACCENT,
    primaryContainer = Pal.ACCENT.copy(alpha = 0.18f),
    onPrimaryContainer = Pal.TEXT,
    secondary = Pal.ACCENT,
    onSecondary = Pal.ON_ACCENT,
    secondaryContainer = Pal.ACCENT.copy(alpha = 0.18f),
    onSecondaryContainer = Pal.TEXT,
    background = Pal.BG,
    onBackground = Pal.TEXT,
    surface = Pal.PANEL,
    onSurface = Pal.TEXT,
    surfaceVariant = Pal.CARD,
    onSurfaceVariant = Pal.TEXT_DIM,
    surfaceContainerLowest = Pal.FIELD,
    surfaceContainerLow = Pal.PANEL,
    surfaceContainer = Pal.PANEL,
    surfaceContainerHigh = Pal.CARD,
    surfaceContainerHighest = Pal.CARD,
    outline = Pal.BORDER,
    outlineVariant = Pal.BORDER,
    error = Pal.BAD,
    onError = Pal.ON_ACCENT,
)

private val base = Typography()
private val type = base.copy(
    titleLarge = base.titleLarge.copy(fontWeight = FontWeight.SemiBold),
    titleMedium = base.titleMedium.copy(fontWeight = FontWeight.SemiBold),
    labelSmall = TextStyle(fontSize = 11.sp, letterSpacing = 0.4.sp, fontWeight = FontWeight.Medium),
)

@Composable
fun SyncTubeTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = scheme, typography = type, content = content)
}
