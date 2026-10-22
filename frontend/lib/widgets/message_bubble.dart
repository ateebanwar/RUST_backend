import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_markdown/flutter_markdown.dart';
import '../models/conversation.dart';
import '../theme/app_theme.dart';

/// Message bubble rendering user prompts and assistant responses
/// with Markdown, code block copy, and Drive persistence warning banner.
class MessageBubble extends StatelessWidget {
  final SyncMessage message;
  final VoidCallback? onRetry;

  const MessageBubble({
    super.key,
    required this.message,
    this.onRetry,
  });

  void _copyToClipboard(BuildContext context, String text) {
    Clipboard.setData(ClipboardData(text: text));
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(
        content: Text('Copied to clipboard'),
        duration: Duration(seconds: 2),
        backgroundColor: AppTheme.darkSurfaceVariant,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    if (message.isUser) {
      return _buildUserBubble(context);
    } else {
      return _buildAssistantBubble(context);
    }
  }

  Widget _buildUserBubble(BuildContext context) {
    return Align(
      alignment: Alignment.centerRight,
      child: Container(
        margin: const EdgeInsets.only(top: 8, bottom: 8, left: 48, right: 12),
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        decoration: BoxDecoration(
          color: AppTheme.darkSurfaceVariant,
          borderRadius: BorderRadius.circular(20),
        ),
        child: SelectableText(
          message.content,
          style: const TextStyle(
            color: AppTheme.textPrimaryDark,
            fontSize: 15,
            height: 1.45,
          ),
        ),
      ),
    );
  }

  Widget _buildAssistantBubble(BuildContext context) {
    final isSaveFailed = !message.isSaved || message.saveError != null;

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Header / Provider & Model tag if available
          Row(
            children: [
              Container(
                width: 24,
                height: 24,
                decoration: const BoxDecoration(
                  color: AppTheme.darkSurfaceVariant,
                  shape: BoxShape.circle,
                ),
                child: const Icon(
                  Icons.auto_awesome,
                  size: 13,
                  color: AppTheme.textPrimaryDark,
                ),
              ),
              const SizedBox(width: 8),
              Text(
                message.modelId ?? 'AI Assistant',
                style: const TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                  color: AppTheme.textSecondaryDark,
                ),
              ),
              const Spacer(),
              // Copy message action
              if (message.content.isNotEmpty && !message.isStreaming)
                IconButton(
                  icon: const Icon(Icons.copy, size: 14, color: AppTheme.textMutedDark),
                  onPressed: () => _copyToClipboard(context, message.content),
                  tooltip: 'Copy response',
                  padding: EdgeInsets.zero,
                  constraints: const BoxConstraints(),
                ),
            ],
          ),
          const SizedBox(height: 8),

          // Message content with Markdown support
          if (message.content.isEmpty && message.isStreaming)
            const Padding(
              padding: EdgeInsets.symmetric(vertical: 8),
              child: Row(
                children: [
                  SizedBox(
                    width: 14,
                    height: 14,
                    child: CircularProgressIndicator(
                      strokeWidth: 2,
                      color: AppTheme.textSecondaryDark,
                    ),
                  ),
                  SizedBox(width: 8),
                  Text(
                    'Thinking...',
                    style: TextStyle(fontSize: 13, color: AppTheme.textSecondaryDark),
                  ),
                ],
              ),
            )
          else
            MarkdownBody(
              data: message.content,
              selectable: true,
              styleSheet: MarkdownStyleSheet(
                p: const TextStyle(
                  color: AppTheme.textPrimaryDark,
                  fontSize: 15,
                  height: 1.55,
                ),
                code: const TextStyle(
                  backgroundColor: AppTheme.darkSurfaceVariant,
                  color: AppTheme.textPrimaryDark,
                  fontFamily: 'monospace',
                  fontSize: 13,
                ),
                codeblockDecoration: BoxDecoration(
                  color: const Color(0xFF1E1E1E),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(color: AppTheme.darkBorder, width: 1),
                ),
                codeblockPadding: const EdgeInsets.all(12),
                blockquoteDecoration: BoxDecoration(
                  border: const Border(
                    left: BorderSide(color: AppTheme.textMutedDark, width: 3),
                  ),
                  color: AppTheme.darkSurfaceVariant.withOpacity(0.5),
                ),
              ),
            ),

          // Google Drive Save Failure Banner
          if (isSaveFailed) ...[
            const SizedBox(height: 12),
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
              decoration: BoxDecoration(
                color: AppTheme.statusError.withOpacity(0.12),
                borderRadius: BorderRadius.circular(10),
                border: Border.all(
                  color: AppTheme.statusError.withOpacity(0.4),
                  width: 1,
                ),
              ),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Icon(
                    Icons.warning_amber_rounded,
                    color: AppTheme.statusError,
                    size: 18,
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        const Text(
                          'Response generated, but saving to Google Drive failed.',
                          style: TextStyle(
                            fontSize: 13,
                            fontWeight: FontWeight.w600,
                            color: AppTheme.statusError,
                          ),
                        ),
                        if (message.saveError != null) ...[
                          const SizedBox(height: 2),
                          Text(
                            message.saveError!,
                            style: TextStyle(
                              fontSize: 12,
                              color: AppTheme.textPrimaryDark.withOpacity(0.85),
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                  if (onRetry != null && message.isRetryable)
                    TextButton.icon(
                      onPressed: onRetry,
                      icon: const Icon(Icons.refresh, size: 14, color: AppTheme.textPrimaryDark),
                      label: const Text(
                        'Retry',
                        style: TextStyle(fontSize: 12, color: AppTheme.textPrimaryDark),
                      ),
                      style: TextButton.styleFrom(
                        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                        backgroundColor: AppTheme.darkSurfaceVariant,
                        minimumSize: Size.zero,
                        tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                      ),
                    ),
                ],
              ),
            ),
          ],
        ],
      ),
    );
  }
}
