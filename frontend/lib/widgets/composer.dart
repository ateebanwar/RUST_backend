import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../services/speech_service.dart';
import '../state/app_state.dart';
import '../theme/app_theme.dart';
import 'attachment_sheet.dart';
import 'think_modal.dart';

/// ChatGPT Android-inspired message composer component.
class MessageComposer extends StatefulWidget {
  final AppState appState;
  final SpeechService speechService;
  final VoidCallback? onDriveFilesTap;

  const MessageComposer({
    super.key,
    required this.appState,
    required this.speechService,
    this.onDriveFilesTap,
  });

  @override
  State<MessageComposer> createState() => _MessageComposerState();
}

class _MessageComposerState extends State<MessageComposer> {
  final TextEditingController _textController = TextEditingController();
  final FocusNode _focusNode = FocusNode();
  bool _hasText = false;

  @override
  void initState() {
    super.initState();
    _textController.addListener(_onTextChanged);
    widget.speechService.addListener(_onSpeechChanged);
  }

  void _onTextChanged() {
    final hasContent = _textController.text.trim().isNotEmpty;
    if (hasContent != _hasText) {
      setState(() {
        _hasText = hasContent;
      });
    }
  }

  void _onSpeechChanged() {
    if (widget.speechService.recognizedText.isNotEmpty) {
      setState(() {
        _textController.text = widget.speechService.recognizedText;
        _textController.selection = TextSelection.fromPosition(
          TextPosition(offset: _textController.text.length),
        );
      });
    }
  }

  void _handleSend() {
    final text = _textController.text.trim();
    if (text.isEmpty && widget.appState.pendingAttachments.isEmpty) return;
    if (widget.appState.isStreaming) return;

    _textController.clear();
    widget.appState.sendMessage(text);
  }

  void _toggleMic() {
    if (widget.speechService.isListening) {
      widget.speechService.stopListening();
    } else {
      widget.speechService.startListening(
        onResult: (text) {
          if (text.isNotEmpty) {
            setState(() {
              _textController.text = text;
            });
          }
        },
        onError: (err) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(
              content: Text(err),
              backgroundColor: AppTheme.darkSurfaceVariant,
            ),
          );
        },
      );
    }
  }

  @override
  void dispose() {
    _textController.removeListener(_onTextChanged);
    widget.speechService.removeListener(_onSpeechChanged);
    _textController.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final isStreaming = widget.appState.isStreaming;
    final isListening = widget.speechService.isListening;
    final thinkMode = widget.appState.selectedThinkMode;
    final attachments = widget.appState.pendingAttachments;

    return Container(
      padding: const EdgeInsets.fromLTRB(12, 4, 12, 12),
      color: Colors.transparent,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Attachment preview chips
          if (attachments.isNotEmpty)
            Container(
              height: 40,
              margin: const EdgeInsets.only(bottom: 8),
              child: ListView.separated(
                scrollDirection: Axis.horizontal,
                itemCount: attachments.length,
                separatorBuilder: (_, _) => const SizedBox(width: 8),
                itemBuilder: (context, index) {
                  final att = attachments[index];
                  return Container(
                    padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
                    decoration: BoxDecoration(
                      color: AppTheme.darkSurfaceVariant,
                      borderRadius: BorderRadius.circular(16),
                      border: Border.all(color: AppTheme.darkBorder, width: 1),
                    ),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        const Icon(Icons.attach_file, size: 14, color: AppTheme.textSecondaryDark),
                        const SizedBox(width: 6),
                        ConstrainedBox(
                          constraints: const BoxConstraints(maxWidth: 140),
                          child: Text(
                            att.name,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(
                              fontSize: 12,
                              color: AppTheme.textPrimaryDark,
                            ),
                          ),
                        ),
                        const SizedBox(width: 6),
                        InkWell(
                          onTap: () => widget.appState.removeAttachment(index),
                          child: const Icon(Icons.close, size: 14, color: AppTheme.textMutedDark),
                        ),
                      ],
                    ),
                  );
                },
              ),
            ),

          // Main composer bar
          Container(
            decoration: BoxDecoration(
              color: AppTheme.darkSurfaceVariant,
              borderRadius: BorderRadius.circular(26),
              border: Border.all(color: AppTheme.darkBorder, width: 1),
            ),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                // Plus button for attachments
                IconButton(
                  icon: const Icon(Icons.add, color: AppTheme.textSecondaryDark, size: 22),
                  onPressed: () {
                    AttachmentSheet.show(
                      context,
                      onFileSelected: (file) => widget.appState.addAttachment(file),
                      onDriveFilesTap: widget.onDriveFilesTap,
                    );
                  },
                  tooltip: 'Add attachments',
                  padding: const EdgeInsets.all(12),
                  constraints: const BoxConstraints(),
                ),

                // Text input field
                Expanded(
                  child: KeyboardListener(
                    focusNode: FocusNode(),
                    onKeyEvent: (event) {
                      if (event is KeyDownEvent &&
                          event.logicalKey == LogicalKeyboardKey.enter &&
                          !HardwareKeyboard.instance.isShiftPressed) {
                        _handleSend();
                      }
                    },
                    child: TextField(
                      controller: _textController,
                      focusNode: _focusNode,
                      maxLines: 6,
                      minLines: 1,
                      textCapitalization: TextCapitalization.sentences,
                      style: const TextStyle(
                        color: AppTheme.textPrimaryDark,
                        fontSize: 15,
                        height: 1.4,
                      ),
                      decoration: InputDecoration(
                        hintText: isListening
                            ? 'Listening...'
                            : 'Message AI Assistant...',
                        hintStyle: TextStyle(
                          color: isListening
                              ? AppTheme.accentGreen
                              : AppTheme.textMutedDark,
                          fontSize: 15,
                        ),
                        fillColor: Colors.transparent,
                        contentPadding: const EdgeInsets.symmetric(
                          horizontal: 4,
                          vertical: 12,
                        ),
                        border: InputBorder.none,
                        enabledBorder: InputBorder.none,
                        focusedBorder: InputBorder.none,
                      ),
                    ),
                  ),
                ),

                // Think button
                Padding(
                  padding: const EdgeInsets.only(bottom: 6),
                  child: Tooltip(
                    message: 'Thinking Mode: ${thinkMode.label}',
                    child: InkWell(
                      onTap: () {
                        ThinkModal.show(
                          context,
                          currentMode: widget.appState.selectedThinkMode,
                          onSelect: (mode) => widget.appState.setThinkMode(mode),
                        );
                      },
                      borderRadius: BorderRadius.circular(16),
                      child: Container(
                        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
                        decoration: BoxDecoration(
                          color: thinkMode.id != 'standard'
                              ? AppTheme.darkBorder
                              : Colors.transparent,
                          borderRadius: BorderRadius.circular(16),
                        ),
                        child: Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            Icon(
                              Icons.psychology_outlined,
                              size: 16,
                              color: thinkMode.id != 'standard'
                                  ? AppTheme.accentGreen
                                  : AppTheme.textSecondaryDark,
                            ),
                            if (thinkMode.id != 'standard') ...[
                              const SizedBox(width: 4),
                              Text(
                                thinkMode.id == 'deep' ? 'Deep' : 'Auto',
                                style: const TextStyle(
                                  fontSize: 11,
                                  fontWeight: FontWeight.w600,
                                  color: AppTheme.accentGreen,
                                ),
                              ),
                            ],
                          ],
                        ),
                      ),
                    ),
                  ),
                ),

                // Microphone button
                Padding(
                  padding: const EdgeInsets.only(bottom: 4),
                  child: IconButton(
                    icon: Icon(
                      isListening ? Icons.mic : Icons.mic_none,
                      color: isListening ? AppTheme.accentGreen : AppTheme.textSecondaryDark,
                      size: 20,
                    ),
                    onPressed: _toggleMic,
                    tooltip: isListening ? 'Stop listening' : 'Voice input',
                    padding: const EdgeInsets.all(8),
                    constraints: const BoxConstraints(),
                  ),
                ),

                // Send or Stop button
                Padding(
                  padding: const EdgeInsets.only(right: 6, bottom: 6),
                  child: isStreaming
                      ? Container(
                          width: 32,
                          height: 32,
                          decoration: const BoxDecoration(
                            color: Colors.white,
                            shape: BoxShape.circle,
                          ),
                          child: IconButton(
                            icon: const Icon(Icons.stop, color: Colors.black, size: 16),
                            onPressed: () => widget.appState.stopStreaming(),
                            tooltip: 'Stop generating',
                            padding: EdgeInsets.zero,
                          ),
                        )
                      : Container(
                          width: 32,
                          height: 32,
                          decoration: BoxDecoration(
                            color: (_hasText || attachments.isNotEmpty)
                                ? Colors.white
                                : AppTheme.darkBorder,
                            shape: BoxShape.circle,
                          ),
                          child: IconButton(
                            icon: Icon(
                              Icons.arrow_upward,
                              color: (_hasText || attachments.isNotEmpty)
                                  ? Colors.black
                                  : AppTheme.textMutedDark,
                              size: 18,
                            ),
                            onPressed: (_hasText || attachments.isNotEmpty)
                                ? _handleSend
                                : null,
                            tooltip: 'Send message',
                            padding: EdgeInsets.zero,
                          ),
                        ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
